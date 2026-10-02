// SPDX-License-Identifier: MPL-2.0
//! User-triggered utility workers; completions return through the normal actor boundary.
use super::*;
use bareline_app::data_tools::{self, DataTool, Indent, ToolOutput};
use bareline_app::utilities::{self as core, ExportFormat, HashAlgorithm, Rgb, Transform};
use bareline_diff::CancelToken;
use bareline_document::{DocumentSnapshot, EditTransaction, TextOffset};
use bareline_platform::LocalFileSystem;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

enum UtilityResult {
    Text(String),
    Edit(DocumentSnapshot, EditTransaction),
    PagedEdit(bareline_document::paged::PagedSnapshot, EditTransaction),
    /// A JSON or XML tool found invalid text: where its caret goes, and why.
    Located(LocatedSource, TextOffset, String),
    /// Hex View's read-only dump of original bytes: the tab label and text.
    Hex(String, String),
}
/// The document a JSON or XML tool read, captured when it started.
enum LocatedSource {
    Resident(DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
/// Hit id of the XPath prompt's expression field (not a command).
const XPATH_FIELD: &str = "utilities.xpathField";
/// Accessibility id of that field.
const XPATH_FIELD_NODE: u64 = 89_998;
/// Largest paged selection a JSON or XML tool rewrites, like the other
/// paged transformations; validation and XPath read up to the core cap.
const PAGED_EDIT_BYTES: usize = 1024 * 1024;
/// Which print option is currently showing its choice list (dropdown).
#[derive(Clone, Copy, PartialEq)]
enum PrintField {
    Font,
    Size,
    Margins,
}
const PRINT_FONTS: [&str; 4] = ["Cascadia Mono", "Consolas", "Courier New", "Segoe UI"];
const PRINT_SIZES: [f64; 8] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0];
const PRINT_MARGINS: [f64; 4] = [6.0, 12.0, 18.0, 24.0];
/// Pages the print preview lays out; printing itself is not bounded.
const PREVIEW_PAGES: usize = 20;
/// Source lines and bytes per line captured for the preview, so opening Print
/// never walks or copies a large document.
const PREVIEW_SOURCE_LINES: usize = 1200;
const PREVIEW_LINE_BYTES: usize = 1024;
/// Rows the preview strip shows at once.
const PREVIEW_VISIBLE_ROWS: usize = 4;
/// The preview as displayed rows: (page index, text, header or footer).
struct PreviewCache {
    options: bareline_platform::printing::PrintOptions,
    lines: Vec<(usize, String, bool)>,
    pages: usize,
    truncated: bool,
}
/// Stable hit/accessibility ids for the open dropdown's rows (index into the
/// active field's choices). Static so they fit the hit table's `&'static str`.
const PICK_IDS: [&str; 8] = [
    "utilities.pick0",
    "utilities.pick1",
    "utilities.pick2",
    "utilities.pick3",
    "utilities.pick4",
    "utilities.pick5",
    "utilities.pick6",
    "utilities.pick7",
];
pub(super) struct UtilitiesRuntime {
    pending: Option<mpsc::Receiver<Result<UtilityResult, String>>>,
    cancel: CancelToken,
    print_cancel: Arc<AtomicBool>,
    pub(super) print_options: bareline_platform::printing::PrintOptions,
    pub(super) result: Option<String>,
    open: bool,
    options_open: bool,
    print_popup: Option<PrintField>,
    selection_only: bool,
    /// Start of the document, or of the selection, that the print preview lays
    /// out; `None` for a paged document, which is not held in memory.
    preview_source: Option<Vec<(usize, String)>>,
    preview: Option<PreviewCache>,
    /// First preview row shown, counted across pages.
    preview_row: usize,
    preview_bounds: Option<bareline_renderer::Rect>,
    focus: usize,
    hits: Vec<(bareline_renderer::Rect, &'static str)>,
    progress: Arc<std::sync::atomic::AtomicU64>,
    progress_seen: u64,
    total: usize,
    last_command: String,
    /// The dialog shows the XPath prompt (BIZ-04).
    xpath_open: bool,
    xpath_field: bareline_ui::text_field::TextField,
}
impl Default for UtilitiesRuntime {
    fn default() -> Self {
        Self {
            pending: None,
            cancel: CancelToken::default(),
            print_cancel: Arc::new(AtomicBool::new(false)),
            print_options: Default::default(),
            result: None,
            open: false,
            options_open: false,
            print_popup: None,
            selection_only: false,
            preview_source: None,
            preview: None,
            preview_row: 0,
            preview_bounds: None,
            focus: 0,
            hits: Vec::new(),
            progress: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            progress_seen: 0,
            total: 0,
            last_command: String::new(),
            xpath_open: false,
            xpath_field: Default::default(),
        }
    }
}
impl Drop for UtilitiesRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.print_cancel.store(true, Ordering::Release);
    }
}
pub(super) fn register(registry: &mut bareline_commands::CommandRegistry) {
    core::register_commands(registry);
    for (id, title) in [
        ("utilities.cancel", "Cancel utility operation"),
        ("utilities.copyResult", "Copy utility result"),
        ("utilities.print", "Print document…"),
        ("utilities.printSelection", "Print selection…"),
        ("utilities.printNow", "Choose printer and print…"),
        ("utilities.dismiss", "Close utility dialog"),
        ("utilities.printHeader", "Print header"),
        ("utilities.printFooter", "Print footer"),
        ("utilities.printNumbers", "Print line numbers"),
        ("utilities.printSyntax", "Print syntax colors"),
        ("utilities.printFont", "Print font family"),
        ("utilities.printSize", "Print font size"),
        ("utilities.printMargins", "Print margins"),
        ("utilities.printRange", "Print selection only"),
        ("utilities.xpathRun", "Run XPath query"),
    ] {
        let id = bareline_commands::CommandId(id);
        // Print carries Ctrl+P so the default keymap binds it (there is no static list).
        let shortcut = if id.0 == "utilities.print" { "Ctrl+P" } else { "" };
        let registered = registry.register(bareline_commands::CommandSpec {
            id,
            title,
            category: "Utilities",
            shortcut,
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
    }
    // Controls that only act inside the open print or result dialog: its choice
    // lists, the per-job selection toggle, Copy and Close, and the XPath
    // prompt's Run button (UI-04).
    for id in [
        "utilities.copyResult",
        "utilities.dismiss",
        "utilities.printFont",
        "utilities.printSize",
        "utilities.printMargins",
        "utilities.printRange",
        "utilities.xpathRun",
    ] {
        let _ = registry.update_presentation(bareline_commands::CommandId(id), |meta| meta.internal = true);
    }
}
impl UtilitiesRuntime {
    /// Check marks for the File ▸ Save and Print ▸ Print Options toggles.
    pub(super) fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        for (id, checked) in [
            ("utilities.printHeader", self.print_options.header),
            ("utilities.printFooter", self.print_options.footer),
            ("utilities.printNumbers", self.print_options.line_numbers),
            ("utilities.printSyntax", self.print_options.syntax_colors),
        ] {
            context
                .states
                .entry(bareline_commands::CommandId(id))
                .or_default()
                .checked = checked;
        }
    }
}
impl Shell {
    pub(super) fn utilities_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        if !self.utilities.open {
            return Vec::new();
        }
        let mut nodes: Vec<_> = self
            .utilities
            .hits
            .iter()
            .filter_map(|(bounds, id)| {
                if let Some(rest) = id.strip_prefix("utilities.pick") {
                    let index = rest.parse::<usize>().ok()?;
                    let field = self.utilities.print_popup?;
                    let (name, selected) = match field {
                        PrintField::Font => {
                            let n = PRINT_FONTS.get(index)?;
                            (
                                (*n).to_string(),
                                self.utilities.print_options.font_family.as_str() == *n,
                            )
                        }
                        PrintField::Size => {
                            let v = PRINT_SIZES.get(index)?;
                            (
                                format!("{v} pt"),
                                (*v - self.utilities.print_options.font_size_pt).abs() < 0.01,
                            )
                        }
                        PrintField::Margins => {
                            let v = PRINT_MARGINS.get(index)?;
                            (
                                format!("{v} mm"),
                                (*v - self.utilities.print_options.margin_mm).abs() < 0.01,
                            )
                        }
                    };
                    return Some(AccessibilityNode {
                        id: 86_000 + index as u64,
                        parent: 1,
                        role: AccessibilityRole::ListItem,
                        name,
                        value: None,
                        bounds: [
                            bounds.x as f64,
                            bounds.y as f64,
                            bounds.width as f64,
                            bounds.height as f64,
                        ],
                        disabled: false,
                        selected,
                        expanded: None,
                        focusable: true,
                        invokable: true,
                        position_in_set: None,
                        size_of_set: None,
                    });
                }
                let (index, command) = self
                    .app
                    .commands
                    .entries()
                    .enumerate()
                    .find(|(_, command)| command.id.0 == *id)?;
                let checked = match *id {
                    "utilities.printHeader" => Some(self.utilities.print_options.header),
                    "utilities.printFooter" => Some(self.utilities.print_options.footer),
                    "utilities.printNumbers" => Some(self.utilities.print_options.line_numbers),
                    "utilities.printSyntax" => Some(self.utilities.print_options.syntax_colors),
                    "utilities.printRange" => Some(self.utilities.selection_only),
                    _ => None,
                };
                let value = match *id {
                    "utilities.printFont" => Some(self.utilities.print_options.font_family.clone()),
                    "utilities.printSize" => Some(format!("{} pt", self.utilities.print_options.font_size_pt)),
                    "utilities.printMargins" => Some(format!("{} mm", self.utilities.print_options.margin_mm)),
                    _ => checked.map(|v| if v { "On" } else { "Off" }.into()),
                };
                Some(AccessibilityNode {
                    id: 85_000 + index as u64,
                    parent: 1,
                    role: if checked.is_some() {
                        AccessibilityRole::Checkbox
                    } else {
                        AccessibilityRole::Button
                    },
                    name: command.title.into(),
                    value,
                    bounds: [
                        bounds.x as f64,
                        bounds.y as f64,
                        bounds.width as f64,
                        bounds.height as f64,
                    ],
                    disabled: false,
                    selected: checked.unwrap_or(false),
                    expanded: None,
                    focusable: true,
                    invokable: true,
                    position_in_set: None,
                    size_of_set: None,
                })
            })
            .collect();
        if self.utilities.xpath_open
            && let Some((bounds, _)) = self.utilities.hits.iter().find(|(_, id)| *id == XPATH_FIELD)
        {
            nodes.push(AccessibilityNode {
                id: XPATH_FIELD_NODE,
                parent: 1,
                role: AccessibilityRole::TextField,
                name: "XPath expression".into(),
                value: Some(self.utilities.xpath_field.value().into()),
                bounds: [
                    bounds.x as f64,
                    bounds.y as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                ],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: true,
                invokable: false,
                position_in_set: None,
                size_of_set: None,
            });
        }
        if !self.utilities.options_open {
            nodes.push(AccessibilityNode {
                id: 89_999,
                parent: 1,
                role: AccessibilityRole::Status,
                name: "Utility result and progress".into(),
                value: Some(if self.utilities.pending.is_some() {
                    format!(
                        "{} of {} bytes processed",
                        self.utilities.progress.load(Ordering::Acquire),
                        self.utilities.total
                    )
                } else {
                    self.utilities.result.clone().unwrap_or_default()
                }),
                bounds: [0.0; 4],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
                position_in_set: None,
                size_of_set: None,
            });
        }
        nodes
    }
    pub(super) fn utilities_accessibility_focus(&self) -> Option<u64> {
        if !self.utilities.open {
            return None;
        }
        let (_, id) = self.utilities.hits.get(self.utilities.focus)?;
        if *id == XPATH_FIELD {
            return Some(XPATH_FIELD_NODE);
        }
        if let Some(rest) = id.strip_prefix("utilities.pick") {
            return rest.parse::<usize>().ok().map(|i| 86_000 + i as u64);
        }
        self.app
            .commands
            .entries()
            .position(|command| command.id.0 == *id)
            .map(|index| 85_000 + index as u64)
    }
    pub(super) fn utilities_accessibility(
        &mut self,
        el: &ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            AccessibilityAction::SetValue { id, value } => return self.utilities_set_value(*id, value),
            _ => return false,
        };
        if !self
            .utilities_accessibility_nodes()
            .iter()
            .any(|node| node.id == id && node.focusable)
        {
            return false;
        }
        if id == XPATH_FIELD_NODE {
            if let Some(index) = self.utilities.hits.iter().position(|(_, hit)| *hit == XPATH_FIELD) {
                self.utilities.focus = index;
            }
            self.utilities_redraw();
            return true;
        }
        if (86_000..87_000).contains(&id) {
            let index = (id - 86_000) as usize;
            if let Some(pos) = self.utilities.hits.iter().position(|(_, cid)| {
                cid.strip_prefix("utilities.pick").and_then(|r| r.parse::<usize>().ok()) == Some(index)
            }) {
                self.utilities.focus = pos;
            }
            if invoke {
                if let Some(pick) = PICK_IDS.get(index).copied() {
                    self.utilities_dispatch(el, pick);
                }
            }
            self.utilities_redraw();
            return true;
        }
        let Some(command) = self
            .app
            .commands
            .entries()
            .nth((id - 85_000) as usize)
            .map(|command| command.id.0)
        else {
            return false;
        };
        if let Some(index) = self
            .utilities
            .hits
            .iter()
            .position(|(_, candidate)| *candidate == command)
        {
            self.utilities.focus = index;
        }
        if invoke {
            self.utilities_dispatch(el, command);
        }
        self.utilities_redraw();
        true
    }
    /// A screen reader sets the XPath expression; other values are not editable.
    fn utilities_set_value(&mut self, id: u64, value: &str) -> bool {
        if id != XPATH_FIELD_NODE || !self.utilities.open || !self.utilities.xpath_open {
            return false;
        }
        let field = &mut self.utilities.xpath_field;
        field.select_all();
        field.insert(value);
        if let Some(index) = self.utilities.hits.iter().position(|(_, hit)| *hit == XPATH_FIELD) {
            self.utilities.focus = index;
        }
        self.utilities_redraw();
        true
    }
    pub(super) fn utilities_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("utilities.") {
            return false;
        }
        if id == "utilities.dismiss" {
            self.utilities.open = false;
            self.utilities.options_open = false;
            self.utilities.xpath_open = false;
            self.utilities.print_popup = None;
            self.utilities_redraw();
            return true;
        }
        // XPath Query… asks for the expression first; Run XPath query starts it.
        if id == data_tools::XPATH_COMMAND {
            self.utilities.open = true;
            if self.utilities.pending.is_none() {
                self.utilities.options_open = false;
                self.utilities.print_popup = None;
                self.utilities.xpath_open = true;
                self.utilities.last_command = id.into();
                self.utilities.focus = 0;
                self.utilities.xpath_field.select_all();
            }
            self.utilities_redraw();
            return true;
        }
        // A row inside an open dropdown: apply the chosen value and close the list.
        if let Some(rest) = id.strip_prefix("utilities.pick") {
            if let (Ok(index), Some(field)) = (rest.parse::<usize>(), self.utilities.print_popup) {
                match field {
                    PrintField::Font => {
                        if let Some(name) = PRINT_FONTS.get(index) {
                            self.utilities.print_options.font_family = (*name).into();
                        }
                    }
                    PrintField::Size => {
                        if let Some(value) = PRINT_SIZES.get(index) {
                            self.utilities.print_options.font_size_pt = *value;
                        }
                    }
                    PrintField::Margins => {
                        if let Some(value) = PRINT_MARGINS.get(index) {
                            self.utilities.print_options.margin_mm = *value;
                        }
                    }
                }
            }
            self.utilities.print_popup = None;
            self.utilities_redraw();
            return true;
        }
        if matches!(id, "utilities.print" | "utilities.printSelection") {
            self.utilities.open = true;
            self.utilities.options_open = self.utilities.pending.is_none();
            self.utilities.xpath_open = false;
            self.utilities.selection_only = id == "utilities.printSelection";
            self.utilities.print_popup = None;
            self.utilities.focus = 0;
            self.utilities_capture_preview();
            self.utilities_redraw();
            return true;
        }
        // Font family / size / margins open a choice list rather than cycling.
        if matches!(
            id,
            "utilities.printFont" | "utilities.printSize" | "utilities.printMargins"
        ) {
            let field = match id {
                "utilities.printFont" => PrintField::Font,
                "utilities.printSize" => PrintField::Size,
                _ => PrintField::Margins,
            };
            self.utilities.print_popup = if self.utilities.print_popup == Some(field) {
                None
            } else {
                Some(field)
            };
            self.utilities_redraw();
            return true;
        }
        let options = &mut self.utilities.print_options;
        match id {
            "utilities.printHeader" => options.header = !options.header,
            "utilities.printFooter" => options.footer = !options.footer,
            "utilities.printNumbers" => options.line_numbers = !options.line_numbers,
            "utilities.printSyntax" => options.syntax_colors = !options.syntax_colors,
            "utilities.printRange" => self.utilities.selection_only = !self.utilities.selection_only,
            _ => {}
        }
        if matches!(
            id,
            "utilities.printHeader"
                | "utilities.printFooter"
                | "utilities.printNumbers"
                | "utilities.printSyntax"
                | "utilities.printRange"
        ) {
            if id == "utilities.printRange" {
                self.utilities_capture_preview();
            }
            self.utilities_redraw();
            return true;
        }
        if id == "utilities.cancel" {
            self.utilities.cancel.cancel();
            self.utilities.print_cancel.store(true, Ordering::Release);
            return true;
        }
        if id == "utilities.copyResult" {
            if let (Some(platform), Some(result)) = (&self.platform, &self.utilities.result) {
                let _ = platform.set_clipboard_text(result);
            }
            return true;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        if self.utilities.pending.is_some() {
            workspace.message = Some("A utility is running; cancel it before starting another".into());
            return true;
        }
        let Some(editor) = workspace.editors.get(self.app.active) else {
            return true;
        };
        let selection = editor.viewport().selection;
        let snapshot = editor.snapshot().clone();
        let paged = match editor {
            bareline_app::workspace::WorkspaceEditor::Paged(p) => Some((p.read_handle(), p.global_selection())),
            _ => None,
        };
        let (anchor, caret) = paged.as_ref().map_or(
            (TextOffset(selection.anchor), TextOffset(selection.caret)),
            |(_, selection)| *selection,
        );
        let length = paged
            .as_ref()
            .map_or(snapshot.len(), |(source, _)| source.snapshot().len());
        let selected = anchor != caret;
        let range = if selected {
            anchor.min(caret)..anchor.max(caret)
        } else {
            TextOffset(0)..TextOffset(length)
        };
        let language = editor.viewport().language;
        let title = workspace
            .titles()
            .get(self.app.active)
            .cloned()
            .unwrap_or_else(|| "Document".into());
        let algorithm = match id {
            "utilities.md5" => Some(HashAlgorithm::Md5),
            "utilities.sha1" => Some(HashAlgorithm::Sha1),
            "utilities.sha256" => Some(HashAlgorithm::Sha256),
            "utilities.sha512" => Some(HashAlgorithm::Sha512),
            _ => None,
        };
        let transform = match id {
            "utilities.base64Encode" => Some(Transform::Base64Encode),
            "utilities.base64Decode" => Some(Transform::Base64Decode),
            "utilities.urlEncode" => Some(Transform::UrlEncode),
            "utilities.urlDecode" => Some(Transform::UrlDecode),
            _ => None,
        };
        // Built-in JSON, XML and Hex tools (BIZ-04).
        let data_tool = DataTool::from_command(id);
        let xpath = (id == data_tools::XPATH_RUN_COMMAND).then(|| self.utilities.xpath_field.value().to_owned());
        let effective = self.settings.effective();
        let indent = Indent::from_settings(effective.tab_width, effective.insert_spaces);
        // Formatted text uses the document's line ending when the range has none.
        let paged_eol = match editor {
            bareline_app::workspace::WorkspaceEditor::Paged(p) => match p.initial_eol_label() {
                Some("CRLF") => "\r\n",
                Some("CR") => "\r",
                _ => "\n",
            },
            _ => "\n",
        };
        if (transform.is_some() || data_tool.is_some_and(DataTool::edits)) && (editor.read_only() || editor.busy()) {
            workspace.message = Some("The destination is read-only or has an edit pending".into());
            return true;
        }
        let original = if id == data_tools::HEX_COMMAND {
            match workspace.raw_source_descriptor(self.app.active) {
                Ok(Some(source)) => Some(source),
                Ok(None) => {
                    workspace.message =
                        Some("Hex View shows a file's bytes as saved on disk. Save this document first.".into());
                    return true;
                }
                Err(error) => {
                    workspace.message = Some(format!("Hex View is unavailable for this document: {error}"));
                    return true;
                }
            }
        } else {
            None
        };
        let hex_name = title.trim_end_matches(['\u{2022}', '*', '\u{25cf}', ' ']).to_owned();
        let export = match id {
            "utilities.exportHtml" => Some(ExportFormat::Html),
            "utilities.exportRtf" => Some(ExportFormat::Rtf),
            _ => None,
        };
        let printing = id == "utilities.printNow";
        let print_selection = self.utilities.selection_only;
        if printing && print_selection && !selected {
            workspace.message = Some("Select text before printing a selection".into());
            self.utilities.result = workspace.message.clone();
            return true;
        }
        let destination = if let Some(format) = export {
            // Offer the export's own type and extension, named after the document.
            let kind = match format {
                ExportFormat::Html => bareline_platform::SaveFileKind::Html,
                ExportFormat::Rtf => bareline_platform::SaveFileKind::Rtf,
            };
            let options = bareline_platform::SaveDialogOptions::new(kind)
                .named_after(title.trim_end_matches(['\u{2022}', '*', '\u{25cf}', ' ']));
            match self.platform.as_ref().map(|p| p.save_file_with(&options)) {
                Some(Ok(Some(path))) => Some(path),
                Some(Err(e)) => {
                    workspace.message = Some(e);
                    return true;
                }
                _ => return true,
            }
        } else {
            None
        };
        let printer = if printing {
            let chosen = match &self.platform {
                Some(platform) => platform.choose_printer(),
                None => bareline_platform_windows::printing::choose_printer(None),
            };
            match chosen {
                Ok(Some(p)) => Some(p),
                Ok(None) => return true,
                Err(e) => {
                    workspace.message = Some(format!(
                        "Print unavailable: {e}. Try Print again to choose another printer."
                    ));
                    self.utilities.result = workspace.message.clone();
                    self.utilities.open = true;
                    self.utilities.options_open = false;
                    self.utilities.last_command = "utilities.print".into();
                    return true;
                }
            }
        } else {
            None
        };
        if algorithm.is_none()
            && transform.is_none()
            && export.is_none()
            && !printing
            && id != "utilities.statistics"
            && data_tool.is_none()
            && xpath.is_none()
            && original.is_none()
        {
            return false;
        }
        let mut print_options = self.utilities.print_options.clone();
        print_options.title = title;
        print_options.tab_width = self.settings.effective().tab_width.clamp(1, 16) as u8;
        let colors = [
            "syntax.keyword",
            "syntax.string",
            "syntax.number",
            "syntax.comment",
            "syntax.operator",
        ]
        .map(|key| {
            let c = self.settings.theme_color(key).unwrap_or(self.settings.ui_theme().text);
            Rgb((c.0 >> 16) as u8, (c.0 >> 8) as u8, c.0 as u8)
        });
        let background = {
            let c = self
                .settings
                .theme_color("surface.editor")
                .unwrap_or(self.settings.ui_theme().chrome);
            Rgb((c.0 >> 16) as u8, (c.0 >> 8) as u8, c.0 as u8)
        };
        let foreground = {
            let c = self.settings.ui_theme().text;
            Rgb((c.0 >> 16) as u8, (c.0 >> 8) as u8, c.0 as u8)
        };
        print_options.foreground = ((foreground.0 as u32) << 16) | ((foreground.1 as u32) << 8) | foreground.2 as u32;
        print_options.background = ((background.0 as u32) << 16) | ((background.1 as u32) << 8) | background.2 as u32;
        self.utilities.cancel = CancelToken::default();
        self.utilities.print_cancel = Arc::new(AtomicBool::new(false));
        self.utilities.progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        self.utilities.total = original
            .as_ref()
            .map_or(length, |source| source.len().min(data_tools::MAX_HEX_BYTES) as usize);
        self.utilities.progress_seen = 0;
        self.utilities.open = true;
        self.utilities.options_open = false;
        self.utilities.xpath_open = false;
        self.utilities.result = None;
        self.utilities.last_command = id.into();
        let cancel = self.utilities.cancel.clone();
        let print_cancel = self.utilities.print_cancel.clone();
        let progress = self.utilities.progress.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let notify = self.notify.clone();
        let spawned = std::thread::Builder::new()
            .name("bareline-utility".into())
            .spawn(move || {
                let mut previous = 0;
                let mut last = std::time::Instant::now();
                let mut report = |bytes: usize| {
                    progress.store(bytes as u64, Ordering::Release);
                    if bytes.saturating_sub(previous) >= 1024 * 1024
                        || last.elapsed() >= std::time::Duration::from_millis(100)
                    {
                        previous = bytes;
                        last = std::time::Instant::now();
                        notify();
                    }
                };
                let result = (|| -> Result<UtilityResult, String> {
                    if let Some(original) = original {
                        // Hex View reads bounded pages of the original bytes
                        // through the same provenance readers as extensions.
                        let total = original.len();
                        let mut readers = super::extensions::readers::Readers::new(
                            Some(original),
                            None,
                            print_cancel.clone(),
                            std::time::Instant::now() + std::time::Duration::from_secs(120),
                        );
                        let text = data_tools::hex_dump(&hex_name, total, |range| {
                            report(range.end as usize);
                            readers.raw(bareline_extensions_protocol::RawRange {
                                start: range.start,
                                end: range.end,
                            })
                        })?;
                        return Ok(UtilityResult::Hex(format!("{hex_name} (hex)"), text));
                    }
                    if let Some(tool) = data_tool {
                        return Ok(if let Some((source, _)) = paged {
                            let captured = source.snapshot().clone();
                            let limit = if tool.edits() {
                                PAGED_EDIT_BYTES
                            } else {
                                data_tools::MAX_INPUT_BYTES
                            };
                            let text = read_paged(source, range.clone(), limit, &cancel)?;
                            let output = data_tools::run_on_text(&text, range.start, tool, indent, paged_eol)?;
                            data_result(output, LocatedSource::Paged(captured), range)
                        } else {
                            let output = data_tools::run_on_snapshot(&snapshot, range.clone(), tool, indent)?;
                            data_result(output, LocatedSource::Resident(snapshot), range)
                        });
                    }
                    if let Some(prompt) = xpath {
                        return Ok(if let Some((source, _)) = paged {
                            let captured = source.snapshot().clone();
                            let text = read_paged(source, range.clone(), data_tools::MAX_INPUT_BYTES, &cancel)?;
                            let output = data_tools::xpath_on_text(&text, range.start, &prompt)?;
                            data_result(output, LocatedSource::Paged(captured), range)
                        } else {
                            let output = data_tools::xpath_on_snapshot(&snapshot, range.clone(), &prompt)?;
                            data_result(output, LocatedSource::Resident(snapshot), range)
                        });
                    }
                    if let Some(algorithm) = algorithm {
                        let result = if let Some((source, _)) = paged {
                            let reader =
                                core::PagedTextReader::new(source, range, cancel.clone()).map_err(|e| e.to_string())?;
                            core::hash_reader(reader, algorithm, &cancel, |bytes| {
                                report(bytes as usize);
                                true
                            })
                        } else {
                            core::hash_snapshot(&snapshot, range, algorithm, &cancel)
                        }
                        .map_err(|e| e.to_string())?;
                        return Ok(UtilityResult::Text(format!(
                            "{} · {} bytes\n{}",
                            algorithm.label(),
                            result.bytes,
                            result.hexadecimal
                        )));
                    }
                    if let Some(kind) = transform {
                        if let Some((source, _)) = paged {
                            use std::io::Read;
                            if range.end.0 - range.start.0 > 1024 * 1024 {
                                return Err(
                                    "Transformation selection exceeds the 1 MiB staging limit; select a smaller range"
                                        .into(),
                                );
                            }
                            let captured = source.snapshot().clone();
                            let mut raw = String::new();
                            core::PagedTextReader::new(source, range.clone(), cancel.clone())
                                .map_err(|e| e.to_string())?
                                .read_to_string(&mut raw)
                                .map_err(|e| e.to_string())?;
                            let doc = bareline_document::Document::from_utf8(
                                &raw,
                                bareline_document::Budget::new(4 * 1024 * 1024),
                                bareline_document::Budget::new(0),
                            )
                            .map_err(|e| e.to_string())?;
                            let mut transaction = core::transform(
                                &doc.snapshot(),
                                TextOffset(0)..TextOffset(raw.len()),
                                kind,
                                1024 * 1024,
                                &cancel,
                            )
                            .map_err(|e| e.to_string())?;
                            transaction.base_revision = captured.revision;
                            for edit in &mut transaction.edits {
                                edit.range.start.0 += range.start.0;
                                edit.range.end.0 += range.start.0;
                            }
                            return Ok(UtilityResult::PagedEdit(captured, transaction));
                        }
                        let transaction = core::transform(&snapshot, range, kind, 16 * 1024 * 1024, &cancel)
                            .map_err(|e| e.to_string())?;
                        return Ok(UtilityResult::Edit(snapshot, transaction));
                    }
                    if let (Some(format), Some(path)) = (export, destination) {
                        publish_export(&path, |writer| {
                            if let Some((source, _)) = paged {
                                let reader = core::PagedTextReader::new(
                                    source,
                                    TextOffset(0)..TextOffset(length),
                                    cancel.clone(),
                                )?;
                                core::export_reader(
                                    reader,
                                    language,
                                    foreground,
                                    background,
                                    colors,
                                    format,
                                    writer,
                                    &cancel,
                                    &mut report,
                                )
                            } else {
                                core::export_language(
                                    &snapshot, language, foreground, background, colors, format, writer, &cancel,
                                )
                            }
                        })?;
                        return Ok(UtilityResult::Text(format!("Exported {}", path.display())));
                    }
                    if let Some(printer) = printer {
                        let print_language = if print_options.syntax_colors {
                            language
                        } else {
                            bareline_syntax::Language::PlainText
                        };
                        let job = bareline_platform_windows::printing::WindowsPrintJob::start(printer, print_options)
                            .map_err(|e| e.to_string())?;
                        let range = if print_selection {
                            range
                        } else {
                            TextOffset(0)..TextOffset(length)
                        };
                        let summary = if let Some((source, _)) = paged {
                            let reader =
                                core::PagedTextReader::new(source, TextOffset(0)..TextOffset(length), cancel.clone())
                                    .map_err(|e| e.to_string())?;
                            core::print_reader(
                                reader,
                                range,
                                print_language,
                                colors,
                                Box::new(job),
                                &cancel,
                                &print_cancel,
                                &mut report,
                            )?
                        } else {
                            core::print_snapshot(&snapshot, range, print_language, colors, Box::new(job), &print_cancel)
                                .map_err(|e| e.to_string())?
                        };
                        return Ok(UtilityResult::Text(format!(
                            "Sent {} pages / {} lines to the printer",
                            summary.pages, summary.lines
                        )));
                    }
                    let s = if let Some((source, _)) = paged {
                        let revision = source.snapshot().revision;
                        let reader =
                            core::PagedTextReader::new(source, TextOffset(0)..TextOffset(length), cancel.clone())
                                .map_err(|e| e.to_string())?;
                        core::statistics_reader(reader, revision, &cancel, &mut report)
                    } else {
                        core::statistics(&snapshot, 256 * 1024, &cancel, |bytes| {
                            report(bytes);
                            true
                        })
                    }
                    .map_err(|e| e.to_string())?;
                    Ok(UtilityResult::Text(format!(
                        "{} bytes · {} characters · {} graphemes · {} words · {} lines",
                        s.bytes, s.characters, s.graphemes, s.words, s.lines
                    )))
                })();
                let _ = send.send(result);
                notify();
            });
        match spawned {
            Ok(_) => {
                self.utilities.pending = Some(receive);
                workspace.message = Some("Utility running · Cancel utility operation stops the worker".into());
            }
            Err(e) => workspace.message = Some(format!("Could not start utility: {e}")),
        }
        true
    }
    pub(super) fn utilities_pump(&mut self, _el: &ActiveEventLoop) {
        let progress = self.utilities.progress.load(Ordering::Acquire);
        if progress != self.utilities.progress_seen {
            self.utilities.progress_seen = progress;
            self.utilities_redraw();
        }
        let Some(receiver) = &self.utilities.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("Utility worker stopped; retry".into()),
        };
        self.utilities.pending = None;
        self.utilities_finish(result);
    }
    /// Applies a finished worker's result: an edit, a report, a caret
    /// position with its message, or a new Hex View tab.
    fn utilities_finish(&mut self, result: Result<UtilityResult, String>) {
        let applied = self.utilities.applied_message();
        let data_tool = data_tools::is_tool_command(&self.utilities.last_command);
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let message = match result {
            Ok(UtilityResult::Text(text)) => {
                self.utilities.result = Some(text.clone());
                text
            }
            Ok(UtilityResult::Edit(snapshot, transaction)) => match workspace
                .editors
                .iter_mut()
                .find(|e| !e.paged() && e.snapshot().same_document(&snapshot))
            {
                Some(editor) => match editor.apply_prepared(&snapshot, transaction) {
                    Ok(()) => applied,
                    Err(e) => format!("Transformation not applied: {e}"),
                },
                None => "Destination closed; transformation not applied".into(),
            },
            Ok(UtilityResult::PagedEdit(snapshot, transaction)) => {
                match workspace.editors.iter_mut().find_map(|e| match e {
                    bareline_app::workspace::WorkspaceEditor::Paged(p) if p.snapshot().same_document(&snapshot) => {
                        Some(p)
                    }
                    _ => None,
                }) {
                    Some(editor) => match editor.apply_prepared(&snapshot, transaction) {
                        Ok(()) => applied,
                        Err(e) => format!("Transformation not applied: {e}"),
                    },
                    None => "Destination closed; transformation not applied".into(),
                }
            }
            // Invalid JSON or XML: the caret goes to the error while the text
            // there is still the text that was checked.
            Ok(UtilityResult::Located(source, offset, mut text)) => {
                let placed = match &source {
                    LocatedSource::Resident(snapshot) => workspace.editors.iter_mut().find_map(|editor| match editor {
                        bareline_app::workspace::WorkspaceEditor::Resident(surface)
                            if surface.snapshot().same_document(snapshot)
                                && surface.snapshot().revision == snapshot.revision =>
                        {
                            let caret = bareline_editor_surface::Selection {
                                anchor: offset.0,
                                caret: offset.0,
                            };
                            Some(surface.set_selections(caret.into()).is_ok())
                        }
                        _ => None,
                    }),
                    LocatedSource::Paged(snapshot) => workspace.editors.iter_mut().find_map(|editor| match editor {
                        bareline_app::workspace::WorkspaceEditor::Paged(paged)
                            if paged.snapshot().same_document(snapshot)
                                && paged.snapshot().revision == snapshot.revision =>
                        {
                            Some(paged.restore_global_selection(offset, offset, false).is_ok())
                        }
                        _ => None,
                    }),
                };
                if placed != Some(true) {
                    text.push_str(" The document changed since the check, so the caret was not moved.");
                }
                text
            }
            Ok(UtilityResult::Hex(label, text)) => {
                let budget = bareline_document::Budget::new(text.len().saturating_add(1024 * 1024));
                match bareline_document::Document::from_utf8(&text, budget, bareline_document::Budget::new(0))
                    .and_then(|document| workspace.add_snapshot_preview(&document.snapshot(), label.clone()))
                {
                    Ok(index) => {
                        self.app.active = index;
                        self.utilities.open = false;
                        format!("Opened {label}: the file's original bytes, read only")
                    }
                    Err(error) => format!("Hex View could not open its tab: {error}"),
                }
            }
            // JSON, XML and Hex messages are already complete sentences.
            Err(e) if data_tool => e,
            Err(e) => format!("Utility stopped: {e}. Source preserved; retry the command."),
        };
        self.utilities.result = Some(message.clone());
        workspace.message = Some(message);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn utilities_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Capture the start of the document, or of the selection, with the title
    /// and tab width printing will use, for the print preview (APP-20).
    fn utilities_capture_preview(&mut self) {
        self.utilities.preview = None;
        self.utilities.preview_row = 0;
        self.utilities.preview_source = None;
        self.utilities.print_options.tab_width = self.settings.effective().tab_width.clamp(1, 16) as u8;
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Some(title) = workspace.titles().get(self.app.active) {
            self.utilities.print_options.title = title.clone();
        }
        let Some(bareline_app::workspace::WorkspaceEditor::Resident(editor)) = workspace.editors.get(self.app.active)
        else {
            return;
        };
        let snapshot = editor.snapshot();
        let selection = editor.selection;
        let (start, end) = if self.utilities.selection_only && selection.anchor != selection.caret {
            (
                selection.anchor.min(selection.caret),
                selection.anchor.max(selection.caret),
            )
        } else {
            (0, snapshot.len())
        };
        let Ok(first) = snapshot.line_at(TextOffset(start)) else {
            return;
        };
        let mut lines = Vec::new();
        // The same line selection as `print_snapshot`.
        for line in first..snapshot.line_count().min(first.saturating_add(PREVIEW_SOURCE_LINES)) {
            let Ok(range) = snapshot.line_range(line) else {
                break;
            };
            if range.start.0 >= end && start != end {
                break;
            }
            let (from, mut to) = (range.start.0.max(start), range.end.0.min(end));
            if from > to || (from == to && start != end) {
                continue;
            }
            to = to.min(from + PREVIEW_LINE_BYTES);
            while !snapshot.is_boundary(TextOffset(to)) {
                to -= 1;
            }
            let text = snapshot
                .chunks(TextOffset(from)..TextOffset(to))
                .map(|chunks| chunks.collect::<String>())
                .unwrap_or_default();
            lines.push((line + 1, text));
        }
        self.utilities.preview_source = Some(lines);
    }
    pub(super) fn utilities_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.utilities.open {
            return false;
        }
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some((i, (_, id))) = self
                    .utilities
                    .hits
                    .iter()
                    .enumerate()
                    .find(|(_, (r, _))| r.contains(self.pointer))
                {
                    let id = *id;
                    self.utilities.focus = i;
                    if id == XPATH_FIELD {
                        if let Some(renderer) = &self.renderer {
                            let _ =
                                self.utilities
                                    .xpath_field
                                    .click(renderer, self.pointer, self.modifiers.shift_key());
                        }
                        self.utilities_redraw();
                    } else {
                        self.utilities_dispatch(el, id);
                    }
                } else if self.utilities.print_popup.is_some() {
                    self.utilities.print_popup = None;
                    self.utilities_redraw();
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // The XPath prompt's field takes text; Tab, Escape and Enter
                // keep their dialog meaning.
                let editing = self.utilities.xpath_open
                    && self
                        .utilities
                        .hits
                        .get(self.utilities.focus)
                        .is_some_and(|(_, id)| *id == XPATH_FIELD);
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) if editing && self.utilities.xpath_field.composing() => {
                        self.utilities.xpath_field.cancel();
                    }
                    Key::Named(NamedKey::Enter) if editing => {
                        self.utilities_dispatch(el, data_tools::XPATH_RUN_COMMAND);
                    }
                    key if editing && !matches!(key, Key::Named(NamedKey::Escape | NamedKey::Tab)) => {
                        let field = &mut self.utilities.xpath_field;
                        match key {
                            Key::Named(NamedKey::Backspace) => {
                                field.delete(false);
                            }
                            Key::Named(NamedKey::Delete) => {
                                field.delete(true);
                            }
                            Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::ArrowRight) => field.horizontal(true, self.modifiers.shift_key()),
                            Key::Named(NamedKey::Home) => field.edge(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::End) => field.edge(true, self.modifiers.shift_key()),
                            Key::Character(v) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                                match v.to_lowercase().as_str() {
                                    "a" => field.select_all(),
                                    "c" | "x" => {
                                        if let Some(platform) = &self.platform
                                            && platform.set_clipboard_text(field.selected()).is_ok()
                                            && v.eq_ignore_ascii_case("x")
                                        {
                                            field.insert("");
                                        }
                                    }
                                    "v" => {
                                        if let Some(platform) = &self.platform
                                            && let Ok(Some(value)) =
                                                platform.clipboard_text_within(bareline_ui::text_field::LIMIT)
                                        {
                                            field.commit(&value);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                                if let Some(value) = &event.text {
                                    field.insert(value);
                                }
                            }
                            _ => {}
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        if self.utilities.print_popup.is_some() {
                            self.utilities.print_popup = None;
                        } else {
                            self.utilities.open = false;
                            self.utilities.options_open = false;
                            self.utilities.xpath_open = false;
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        let count = self.utilities.hits.len();
                        if count > 0 {
                            self.utilities.focus = (self.utilities.focus + 1) % count;
                        }
                    }
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                        if let Some((_, id)) = self.utilities.hits.get(self.utilities.focus) {
                            let id = *id;
                            self.utilities_dispatch(el, id);
                        }
                    }
                    Key::Named(key @ (NamedKey::PageDown | NamedKey::PageUp)) if self.utilities.options_open => {
                        self.utilities.page_preview(*key == NamedKey::PageDown);
                    }
                    _ => {}
                }
                self.utilities_redraw();
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.utilities.options_open
                    && self
                        .utilities
                        .preview_bounds
                        .is_some_and(|bounds| bounds.contains(self.pointer))
                {
                    let rows = match delta {
                        MouseScrollDelta::LineDelta(_, y) => -(y.round() as isize),
                        MouseScrollDelta::PixelDelta(point) => -((point.y / 20.0).round() as isize),
                    };
                    self.utilities.scroll_preview(rows);
                    self.utilities_redraw();
                }
                true
            }
            WindowEvent::Ime(ime) => {
                if self.utilities.xpath_open {
                    let field = &mut self.utilities.xpath_field;
                    match ime {
                        Ime::Preedit(value, cursor) => field.preedit(value.clone(), *cursor),
                        Ime::Commit(value) => {
                            field.commit(value);
                        }
                        Ime::Disabled => field.cancel(),
                        Ime::Enabled => {}
                    }
                    self.utilities_redraw();
                }
                true
            }
            _ => false,
        }
    }
}
impl UtilitiesRuntime {
    pub(super) fn has_input_focus(&self) -> bool {
        self.open || self.options_open
    }
    /// Lay the captured source out for the current options, once per change.
    fn refresh_preview(&mut self) {
        if self
            .preview
            .as_ref()
            .is_some_and(|cache| cache.options == self.print_options)
        {
            return;
        }
        let Some(source) = &self.preview_source else {
            self.preview = None;
            return;
        };
        let options = &self.print_options;
        let layout = bareline_platform::printing::paginate_preview(
            source.iter().map(|(number, text)| (*number, text.as_str())),
            options,
            PREVIEW_PAGES,
        );
        let mut lines = Vec::new();
        for (page, body) in layout.pages.iter().enumerate() {
            if options.header {
                lines.push((page, options.title.clone(), true));
            }
            for row in &body.rows {
                // The printer's gutter: a right-aligned number and two spaces.
                let text = match (options.line_numbers, row.number) {
                    (true, Some(number)) => format!("{number:>6}  {}", row.text),
                    (true, None) => format!("{:8}{}", "", row.text),
                    (false, _) => row.text.clone(),
                };
                lines.push((page, text, false));
            }
            if options.footer {
                lines.push((page, format!("Page {}", page + 1), true));
            }
        }
        self.preview_row = self.preview_row.min(lines.len().saturating_sub(1));
        self.preview = Some(PreviewCache {
            options: options.clone(),
            lines,
            pages: layout.pages.len(),
            truncated: layout.truncated,
        });
    }
    fn scroll_preview(&mut self, rows: isize) {
        let count = self.preview.as_ref().map_or(0, |cache| cache.lines.len());
        self.preview_row = self
            .preview_row
            .saturating_add_signed(rows)
            .min(count.saturating_sub(PREVIEW_VISIBLE_ROWS));
    }
    /// Show the start of the next or previous page.
    fn page_preview(&mut self, forward: bool) {
        let Some(cache) = &self.preview else {
            return;
        };
        let current = cache.lines.get(self.preview_row).map_or(0, |line| line.0);
        let target = if forward {
            current + 1
        } else {
            current.saturating_sub(1)
        };
        if let Some(row) = cache.lines.iter().position(|line| line.0 == target) {
            self.preview_row = row;
        }
    }
    /// A descriptive title for the result panel, named after the command that
    /// produced it, so the dialog is legible without reading the source.
    fn result_title(&self) -> &'static str {
        match self.last_command.as_str() {
            "utilities.statistics" => "Document Statistics",
            "utilities.md5" => "MD5 Checksum",
            "utilities.sha1" => "SHA-1 Checksum",
            "utilities.sha256" => "SHA-256 Checksum",
            "utilities.sha512" => "SHA-512 Checksum",
            "utilities.base64Encode" => "Base64 Encode",
            "utilities.base64Decode" => "Base64 Decode",
            "utilities.urlEncode" => "URL Encode",
            "utilities.urlDecode" => "URL Decode",
            "utilities.exportHtml" => "Export to HTML",
            "utilities.exportRtf" => "Export to RTF",
            "utilities.printNow" | "utilities.print" | "utilities.printSelection" => "Print",
            "utilities.jsonFormat" => "Format JSON",
            "utilities.jsonMinify" => "Minify JSON",
            "utilities.jsonValidate" => "Validate JSON",
            "utilities.xmlFormat" => "Format XML",
            "utilities.xmlValidate" => "Validate XML",
            data_tools::XPATH_COMMAND | data_tools::XPATH_RUN_COMMAND => "XPath Query",
            data_tools::HEX_COMMAND => "Hex View",
            _ => "Result",
        }
    }
    /// What a finished edit reports: the JSON or XML tool's action, or the
    /// generic transformation.
    fn applied_message(&self) -> String {
        match DataTool::from_command(&self.last_command) {
            Some(tool) => format!("{} as one undoable edit", tool.done()),
            None => "Transformation queued as one undoable edit".into(),
        }
    }
    /// Window coordinates; draw after editor operation translation and before palette.
    /// Returns the XPath field's caret while it has focus, for IME placement.
    pub(super) fn draw(
        &mut self,
        renderer: &mut impl bareline_renderer::TextBackend,
        settings: &settings::SettingsRuntime,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Result<Option<bareline_renderer::Rect>, bareline_renderer::LayoutError> {
        use bareline_renderer::DrawOp;
        use bareline_ui::{rect, text};
        self.hits.clear();
        self.preview_bounds = None;
        if !self.open || !self.xpath_open {
            self.xpath_field.release(renderer);
        }
        if !self.open {
            return Ok(None);
        }
        if self.options_open {
            self.refresh_preview();
        }
        let mut caret = None;
        let theme = settings.ui_theme();
        let w = (width - 32.0).clamp(280.0, 640.0);
        let h = if self.options_open {
            460.0
        } else if self.xpath_open {
            220.0
        } else if self.pending.is_none() && self.last_command == data_tools::XPATH_RUN_COMMAND {
            // Room for a list of matches.
            460.0
        } else {
            300.0
        };
        let x = (width - w) / 2.0;
        let y = ((height - h) / 2.0).max(8.0);
        let panel = rect(x, y, w, h);
        ops.push(DrawOp::FillRounded(panel, theme.elevated, 10.0));
        ops.push(DrawOp::StrokeRounded(panel, theme.border, 10.0, 1.0));
        let title = if self.options_open {
            "Print options"
        } else if self.pending.is_some() {
            "Working…"
        } else {
            self.result_title()
        };
        text(ops, x + 24.0, y + 22.0, title, 20.0, theme.text);
        let mut button = |label: String, id: &'static str, bounds: bareline_renderer::Rect| {
            ops.push(DrawOp::FillRounded(bounds, theme.chrome, 5.0));
            ops.push(DrawOp::StrokeRounded(bounds, theme.border, 5.0, 1.0));
            text(ops, bounds.x + 10.0, bounds.y + 10.0, &label, 13.0, theme.text);
            self.hits.push((bounds, id));
        };
        if self.options_open {
            let options = &self.print_options;
            for (row, (label, id)) in [
                (
                    format!("{} Header", if options.header { "☑" } else { "☐" }),
                    "utilities.printHeader",
                ),
                (
                    format!("{} Footer", if options.footer { "☑" } else { "☐" }),
                    "utilities.printFooter",
                ),
                (
                    format!("{} Line numbers", if options.line_numbers { "☑" } else { "☐" }),
                    "utilities.printNumbers",
                ),
                (
                    format!("{} Syntax colors", if options.syntax_colors { "☑" } else { "☐" }),
                    "utilities.printSyntax",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                button(
                    label,
                    id,
                    rect(
                        x + 24.0 + (row % 2) as f32 * (w / 2.0 - 12.0),
                        y + 65.0 + (row / 2) as f32 * 47.0,
                        w / 2.0 - 36.0,
                        38.0,
                    ),
                );
            }
            button(
                format!("Font: {}", options.font_family),
                "utilities.printFont",
                rect(x + 24.0, y + 169.0, w - 48.0, 38.0),
            );
            button(
                format!("Size: {} pt", options.font_size_pt),
                "utilities.printSize",
                rect(x + 24.0, y + 216.0, w / 2.0 - 36.0, 38.0),
            );
            button(
                format!("Margins: {} mm", options.margin_mm),
                "utilities.printMargins",
                rect(x + w / 2.0 + 12.0, y + 216.0, w / 2.0 - 36.0, 38.0),
            );
            button(
                format!("Range: {}", if self.selection_only { "Selection" } else { "Document" }),
                "utilities.printRange",
                rect(x + 24.0, y + 263.0, w - 48.0, 38.0),
            );
            button(
                "Choose printer and print…".into(),
                "utilities.printNow",
                rect(x + 24.0, y + h - 64.0, w - 160.0, 38.0),
            );
            button(
                "Cancel".into(),
                "utilities.dismiss",
                rect(x + w - 120.0, y + h - 64.0, 96.0, 38.0),
            );
            // The document's own printed composition (header, numbered and
            // wrapped rows, footer) paginated like the print job (APP-20). The
            // wheel scrolls it; Page Up and Page Down step between pages.
            let preview = rect(x + 24.0, y + 310.0, w - 48.0, 80.0);
            self.preview_bounds = Some(preview);
            ops.push(DrawOp::Fill(preview, theme.editor));
            ops.push(DrawOp::StrokeRounded(preview, theme.border, 4.0, 1.0));
            ops.push(DrawOp::PushClip(preview));
            match &self.preview {
                Some(cache) => {
                    let first = self.preview_row.min(cache.lines.len().saturating_sub(1));
                    let page = cache.lines.get(first).map_or(0, |line| line.0);
                    let caption = format!(
                        "Preview \u{b7} page {} of {}{}",
                        page + 1,
                        cache.pages,
                        if cache.truncated { "+" } else { "" }
                    );
                    text(ops, preview.x + 8.0, preview.y + 3.0, &caption, 11.0, theme.muted);
                    for (row, (_, line, chrome)) in
                        cache.lines.iter().skip(first).take(PREVIEW_VISIBLE_ROWS).enumerate()
                    {
                        text(
                            ops,
                            preview.x + 10.0,
                            preview.y + 20.0 + row as f32 * 14.0,
                            line,
                            10.0,
                            if *chrome { theme.muted } else { theme.text },
                        );
                    }
                }
                None => text(
                    ops,
                    preview.x + 8.0,
                    preview.y + 3.0,
                    "Preview is not available for documents opened in paged mode",
                    11.0,
                    theme.muted,
                ),
            }
            ops.push(DrawOp::PopClip);
            // An open dropdown draws over the panel; rows become the hit targets.
            if let Some(field) = self.print_popup {
                let (anchor, labels, current): (bareline_renderer::Rect, Vec<String>, usize) = match field {
                    PrintField::Font => {
                        let cur = PRINT_FONTS
                            .iter()
                            .position(|f| *f == options.font_family.as_str())
                            .unwrap_or(usize::MAX);
                        (
                            rect(x + 24.0, y + 169.0, w - 48.0, 38.0),
                            PRINT_FONTS.iter().map(|f| (*f).to_string()).collect(),
                            cur,
                        )
                    }
                    PrintField::Size => {
                        let cur = PRINT_SIZES
                            .iter()
                            .position(|v| (*v - options.font_size_pt).abs() < 0.01)
                            .unwrap_or(usize::MAX);
                        (
                            rect(x + 24.0, y + 216.0, w / 2.0 - 36.0, 38.0),
                            PRINT_SIZES.iter().map(|v| format!("{v} pt")).collect(),
                            cur,
                        )
                    }
                    PrintField::Margins => {
                        let cur = PRINT_MARGINS
                            .iter()
                            .position(|v| (*v - options.margin_mm).abs() < 0.01)
                            .unwrap_or(usize::MAX);
                        (
                            rect(x + w / 2.0 + 12.0, y + 216.0, w / 2.0 - 36.0, 38.0),
                            PRINT_MARGINS.iter().map(|v| format!("{v} mm")).collect(),
                            cur,
                        )
                    }
                };
                let row_h = 28.0;
                let pw = anchor.width.max(160.0);
                let ph = labels.len() as f32 * row_h + 8.0;
                let px = anchor.x;
                let mut pyy = anchor.y + anchor.height + 2.0;
                if pyy + ph > y + h - 8.0 {
                    pyy = (anchor.y - ph - 2.0).max(y + 8.0);
                }
                let popup = rect(px, pyy, pw, ph);
                ops.push(DrawOp::FillRounded(popup, theme.elevated, 6.0));
                ops.push(DrawOp::StrokeRounded(popup, theme.border, 6.0, 1.0));
                for (i, label) in labels.iter().enumerate() {
                    let rb = rect(px + 4.0, pyy + 4.0 + i as f32 * row_h, pw - 8.0, row_h);
                    if i == current {
                        ops.push(DrawOp::FillRounded(rb, theme.chrome, 4.0));
                    }
                    text(ops, rb.x + 8.0, rb.y + 7.0, label, 13.0, theme.text);
                    self.hits.push((rb, PICK_IDS[i]));
                }
            }
        } else if self.xpath_open {
            button(
                "Run query".into(),
                data_tools::XPATH_RUN_COMMAND,
                rect(x + 24.0, y + h - 64.0, 124.0, 38.0),
            );
            button(
                "Cancel".into(),
                "utilities.dismiss",
                rect(x + w - 120.0, y + h - 64.0, 96.0, 38.0),
            );
            // The expression field comes first in the focus order.
            let field = rect(x + 24.0, y + 64.0, w - 48.0, 30.0);
            self.hits.insert(0, (field, XPATH_FIELD));
            let focused = self.focus == 0;
            let at = self.xpath_field.draw_with_theme(renderer, field, focused, theme, ops)?;
            if focused {
                caret = Some(at);
            }
            for (row, hint) in [
                "Paths such as /root/item, //item[@id='1']/text() or //item/@name.",
                "Bind namespace prefixes after a bar: //a:item | a=urn:example",
            ]
            .into_iter()
            .enumerate()
            {
                text(ops, x + 24.0, y + 108.0 + row as f32 * 18.0, hint, 12.0, theme.muted);
            }
        } else if self.pending.is_some() {
            button(
                "Cancel operation".into(),
                "utilities.cancel",
                rect(x + 24.0, y + h - 64.0, 160.0, 38.0),
            );
            button(
                "Keep working".into(),
                "utilities.dismiss",
                rect(x + w - 160.0, y + h - 64.0, 136.0, 38.0),
            );
        } else {
            button(
                "Copy result".into(),
                "utilities.copyResult",
                rect(x + 24.0, y + h - 64.0, 124.0, 38.0),
            );
            button(
                "Close".into(),
                "utilities.dismiss",
                rect(x + w - 112.0, y + h - 64.0, 88.0, 38.0),
            );
        }
        if !self.options_open && !self.xpath_open {
            let value = if self.pending.is_some() {
                format!(
                    "{} / {} bytes processed",
                    self.progress.load(Ordering::Acquire),
                    self.total
                )
            } else {
                self.result.clone().unwrap_or_default()
            };
            let mut row = 0;
            let limit = ((w - 48.0) / 7.0).max(20.0) as usize;
            // Rows above the buttons; Copy result copies the whole text.
            let rows = ((h - 142.0) / 20.0).max(1.0) as usize;
            for line in value.lines() {
                let chars: Vec<char> = line.chars().collect();
                for part in chars.chunks(limit) {
                    if row >= rows {
                        break;
                    }
                    let part: String = part.iter().collect();
                    text(ops, x + 24.0, y + 70.0 + row as f32 * 20.0, &part, 13.0, theme.text);
                    row += 1;
                }
            }
            if self.pending.is_some() {
                let fraction = self.progress.load(Ordering::Acquire) as f64 / self.total.max(1) as f64;
                ops.push(DrawOp::Fill(rect(x + 24.0, y + 175.0, w - 48.0, 6.0), theme.chrome));
                ops.push(DrawOp::Fill(
                    rect(x + 24.0, y + 175.0, (w - 48.0) * fraction.clamp(0.0, 1.0) as f32, 6.0),
                    theme.focus,
                ));
            }
        }
        if let Some((bounds, _)) = self.hits.get(self.focus) {
            ops.push(DrawOp::StrokeRounded(*bounds, theme.focus, 5.0, 2.0));
        }
        Ok(caret)
    }
}
/// Text of a paged range for a JSON or XML tool, refused above `limit` bytes.
fn read_paged(
    source: bareline_editor_surface::paged_view::PagedReadHandle,
    range: std::ops::Range<TextOffset>,
    limit: usize,
    cancel: &CancelToken,
) -> Result<String, String> {
    use std::io::Read;
    data_tools::check_size(range.end.0.saturating_sub(range.start.0), limit)?;
    let mut text = String::new();
    core::PagedTextReader::new(source, range, cancel.clone())
        .map_err(|e| format!("The text could not be read ({e}). Try again."))?
        .read_to_string(&mut text)
        .map_err(|e| format!("The text could not be read ({e}). Try again."))?;
    Ok(text)
}
/// A JSON, XML or XPath outcome as a worker result: one edit of `range` in
/// the captured document, a report, or the caret position of an error.
fn data_result(output: ToolOutput, source: LocatedSource, range: std::ops::Range<TextOffset>) -> UtilityResult {
    match (output, source) {
        (ToolOutput::Report(text), _) => UtilityResult::Text(text),
        (ToolOutput::Invalid { offset, message }, source) => UtilityResult::Located(source, offset, message),
        (ToolOutput::Replace(text), LocatedSource::Resident(snapshot)) => {
            let transaction = data_tools::replacement(snapshot.revision, range, text);
            UtilityResult::Edit(snapshot, transaction)
        }
        (ToolOutput::Replace(text), LocatedSource::Paged(snapshot)) => {
            let transaction = data_tools::replacement(snapshot.revision, range, text);
            UtilityResult::PagedEdit(snapshot, transaction)
        }
    }
}
fn publish_export(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), core::UtilityError>,
) -> Result<(), String> {
    use std::io::Write;
    let platform = bareline_platform_windows::WindowsFileSystem;
    platform
        .validate_target(path)
        .map_err(|e| format!("Export validate destination: {e}"))?;
    let parent = path.parent().ok_or("Export destination has no parent")?;
    let _guard = platform
        .guard_directory(parent)
        .map_err(|e| format!("Export guard parent: {e}"))?;
    let existed = path.exists();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let stage = parent.join(format!(
        ".bareline-export-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut owned = false;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)
            .map_err(|e| format!("Export create stage: {e}"))?;
        owned = true;
        write(&mut file).map_err(|e| format!("The export could not be written: {e}."))?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Export flush stage: {e}"))?;
        drop(file);
        platform
            .commit(&stage, path, existed)
            .map_err(|e| format!("Export commit destination: {e}"))
    })();
    if result.is_err() && owned {
        let _ = std::fs::remove_file(&stage);
    }
    result
}

#[cfg(test)]
pub(super) fn accessibility_test_setup(shell: &mut Shell, scenario: &str) {
    register(&mut shell.app.commands);
    shell.utilities = UtilitiesRuntime::default();
    match scenario {
        "closed" => {}
        "open" => shell.utilities.open = true,
        "populated" => {
            let snapshot = bareline_document::Document::from_utf8(
                "fixture 🙂",
                bareline_document::Budget::new(4096),
                bareline_document::Budget::new(0),
            )
            .unwrap()
            .snapshot();
            let hash = core::hash_snapshot(
                &snapshot,
                TextOffset(0)..TextOffset(snapshot.len()),
                HashAlgorithm::Sha256,
                &CancelToken::default(),
            )
            .unwrap();
            shell.utilities.open = true;
            shell.utilities.result = Some(format!(
                "{} · {} bytes\n{}",
                HashAlgorithm::Sha256.label(),
                hash.bytes,
                hash.hexadecimal
            ));
        }
        "options" | "focus" | "value" => {
            shell.utilities.open = true;
            shell.utilities.options_open = true;
            if scenario == "focus" {
                shell.utilities.focus = 2;
            }
            if scenario == "value" {
                shell.utilities.print_options.font_size_pt = 12.0;
                shell.utilities.print_options.font_family = "Cascadia Mono".into();
                shell.utilities.selection_only = true;
            }
        }
        "xpath" => {
            shell.utilities.open = true;
            shell.utilities.xpath_open = true;
            shell.utilities.last_command = data_tools::XPATH_COMMAND.into();
            shell.utilities.xpath_field.insert("//item/@name");
        }
        _ => panic!("unknown utilities accessibility fixture: {scenario}"),
    }
    let mut ops = Vec::new();
    shell
        .utilities
        .draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            &shell.settings,
            1000.0,
            800.0,
            &mut ops,
        )
        .unwrap();
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn print_preview_lays_out_the_document_instead_of_sample_text() {
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("alpha\n\tbeta\n".into()));
        for _ in 0..100_000 {
            if !workspace.editors[0].busy() {
                break;
            }
            workspace.pump();
            std::thread::yield_now();
        }
        assert!(!workspace.editors[0].busy());
        shell.workspace = Some(workspace);
        shell.utilities.open = true;
        shell.utilities.options_open = true;
        shell.utilities_capture_preview();
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        shell
            .utilities
            .draw(&mut renderer, &shell.settings, 1000.0, 800.0, &mut ops)
            .unwrap();
        let texts: Vec<&str> = ops
            .iter()
            .filter_map(|op| match op {
                bareline_renderer::DrawOp::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"     1  alpha"), "{texts:?}");
        assert!(texts.contains(&"     2      beta"), "{texts:?}");
        assert!(texts.iter().any(|text| text.contains("page 1 of 1")), "{texts:?}");
        assert!(!texts.iter().any(|text| text.contains("greet")));

        // Options change the layout: without numbers the rows are the bare text.
        shell.utilities.print_options.line_numbers = false;
        ops.clear();
        shell
            .utilities
            .draw(&mut renderer, &shell.settings, 1000.0, 800.0, &mut ops)
            .unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, bareline_renderer::DrawOp::Text { text, .. } if text == "alpha"))
        );
    }
    #[test]
    fn native_export_publishes_new_and_replaces_existing_destination() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "bareline-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("export.html");
        let result = (|| -> Result<(), String> {
            for content in ["<p>Unicode α</p>", "<p>Replacement β</p>"] {
                publish_export(&path, |writer| {
                    writer.write_all(content.as_bytes()).map_err(|_| core::UtilityError::Io)
                })?;
                assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "stage must be consumed");
            }
            Ok(())
        })();
        std::fs::remove_dir_all(&root).unwrap();
        result.unwrap();
    }
    fn settle(workspace: &mut Workspace) {
        for _ in 0..100_000 {
            if !workspace.editors.iter().any(|editor| editor.busy()) {
                break;
            }
            workspace.pump();
            std::thread::yield_now();
        }
        assert!(!workspace.editors.iter().any(|editor| editor.busy()));
    }
    fn text(shell: &Shell, index: usize) -> String {
        let snapshot = shell.workspace.as_ref().unwrap().editors[index].snapshot();
        snapshot
            .read(TextOffset(0)..TextOffset(snapshot.len()), 1 << 20)
            .unwrap()
    }
    /// Runs a JSON tool over document `index` the way the worker does.
    fn check(shell: &Shell, index: usize, tool: DataTool) -> UtilityResult {
        let snapshot = shell.workspace.as_ref().unwrap().editors[index].snapshot().clone();
        let range = TextOffset(0)..TextOffset(snapshot.len());
        let output = data_tools::run_on_snapshot(&snapshot, range.clone(), tool, Indent::Spaces(2)).unwrap();
        data_result(output, LocatedSource::Resident(snapshot), range)
    }
    #[test]
    fn json_tools_edit_once_and_put_the_caret_on_errors() {
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("{\"a\":[1,2]}".into()));
        workspace.editors[1].enqueue(Input::Insert("[1,\n!]".into()));
        settle(&mut workspace);
        shell.workspace = Some(workspace);

        // Format is one undoable edit of the whole document.
        shell.utilities.last_command = "utilities.jsonFormat".into();
        let formatted = check(&shell, 0, DataTool::JsonFormat);
        shell.utilities_finish(Ok(formatted));
        settle(shell.workspace.as_mut().unwrap());
        assert_eq!(text(&shell, 0), "{\n  \"a\": [\n    1,\n    2\n  ]\n}");
        assert_eq!(
            shell.utilities.result.as_deref(),
            Some("Formatted the JSON as one undoable edit")
        );
        shell.workspace.as_mut().unwrap().editors[0].enqueue(Input::Undo);
        settle(shell.workspace.as_mut().unwrap());
        assert_eq!(text(&shell, 0), "{\"a\":[1,2]}", "one Undo reverts the whole format");

        // Validate reports the position and moves the caret to the error.
        shell.utilities.last_command = "utilities.jsonValidate".into();
        let located = check(&shell, 1, DataTool::JsonValidate);
        shell.utilities_finish(Ok(located));
        let message = shell.utilities.result.clone().unwrap();
        assert!(
            message.starts_with("JSON is not valid at line 2, column 1:"),
            "{message}"
        );
        let caret = shell.workspace.as_ref().unwrap().editors[1].viewport().selection;
        assert_eq!((caret.anchor, caret.caret), (4, 4));

        // A result for text that changed since the check leaves the caret alone.
        let stale = check(&shell, 1, DataTool::JsonValidate);
        let editor = &mut shell.workspace.as_mut().unwrap().editors[1];
        editor.enqueue(Input::DocumentHome(false));
        editor.enqueue(Input::Insert(" ".into()));
        settle(shell.workspace.as_mut().unwrap());
        shell.utilities_finish(Ok(stale));
        assert!(
            shell
                .utilities
                .result
                .as_deref()
                .unwrap()
                .contains("caret was not moved")
        );
        assert_eq!(
            shell.workspace.as_ref().unwrap().editors[1].viewport().selection.caret,
            1
        );

        // Tool errors are already sentences; they are shown as they are.
        shell.utilities.last_command = data_tools::XPATH_RUN_COMMAND.into();
        shell.utilities_finish(Err("This XPath query is not supported (x).".into()));
        assert_eq!(
            shell.utilities.result.as_deref(),
            Some("This XPath query is not supported (x).")
        );
    }
    #[test]
    fn hex_view_opens_a_read_only_tab_and_closes_the_dialog() {
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        shell.workspace = Some(workspace);
        shell.utilities.open = true;
        shell.utilities.last_command = data_tools::HEX_COMMAND.into();
        let bytes = [0xff, 0x41, 0x0a];
        let dump = data_tools::hex_dump("a.bin", 3, |range| {
            Ok(bytes[range.start as usize..range.end as usize].to_vec())
        })
        .unwrap();
        shell.utilities_finish(Ok(UtilityResult::Hex("a.bin (hex)".into(), dump.clone())));
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 2);
        assert_eq!(shell.app.active, 1);
        assert!(workspace.editors[1].read_only());
        assert!(!shell.utilities.open);
        assert_eq!(text(&shell, 1), dump);
        assert!(dump.contains("0000000000000000  FF 41 0A"), "{dump}");
    }
    #[test]
    fn xpath_prompt_is_a_named_text_field_screen_readers_can_set() {
        use bareline_platform::accessibility::AccessibilityRole;
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        accessibility_test_setup(&mut shell, "xpath");
        let nodes = shell.utilities_accessibility_nodes();
        let field = nodes
            .iter()
            .find(|node| node.id == XPATH_FIELD_NODE)
            .expect("XPath field");
        assert_eq!(field.role, AccessibilityRole::TextField);
        assert_eq!(field.name, "XPath expression");
        assert_eq!(field.value.as_deref(), Some("//item/@name"));
        assert!(field.focusable && !field.invokable);
        assert_eq!(shell.utilities_accessibility_focus(), Some(XPATH_FIELD_NODE));
        for name in ["Run XPath query", "Close utility dialog"] {
            assert!(
                nodes
                    .iter()
                    .any(|node| node.name == name && node.role == AccessibilityRole::Button),
                "{name}"
            );
        }
        assert!(shell.utilities_set_value(XPATH_FIELD_NODE, "/r/n[2]"));
        assert_eq!(shell.utilities.xpath_field.value(), "/r/n[2]");
        assert!(!shell.utilities_set_value(89_999, "ignored"));
        // Closed, the prompt takes no value.
        shell.utilities.xpath_open = false;
        assert!(!shell.utilities_set_value(XPATH_FIELD_NODE, "x"));
    }
}
