// SPDX-License-Identifier: MPL-2.0
//! User-triggered utility workers; completions return through the normal actor boundary.
use super::*;
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
}
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
    focus: usize,
    hits: Vec<(bareline_renderer::Rect, &'static str)>,
    progress: Arc<std::sync::atomic::AtomicU64>,
    progress_seen: u64,
    total: usize,
    last_command: String,
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
            focus: 0,
            hits: Vec::new(),
            progress: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            progress_seen: 0,
            total: 0,
            last_command: String::new(),
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
    ] {
        let id = bareline_commands::CommandId(id);
        // Print carries Ctrl+P so the default keymap binds it (there is no static list).
        let shortcut = if id.0 == "utilities.print" { "Ctrl+P" } else { "" };
        let _ = registry.register(bareline_commands::CommandSpec {
            id,
            title,
            category: "Utilities",
            shortcut,
            action: Action::Contributed(id),
        });
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
                })
            })
            .collect();
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
            });
        }
        nodes
    }
    pub(super) fn utilities_accessibility_focus(&self) -> Option<u64> {
        if !self.utilities.open {
            return None;
        }
        let (_, id) = self.utilities.hits.get(self.utilities.focus)?;
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
            _ => return false,
        };
        if !self
            .utilities_accessibility_nodes()
            .iter()
            .any(|node| node.id == id && node.focusable)
        {
            return false;
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
    pub(super) fn utilities_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("utilities.") {
            return false;
        }
        if id == "utilities.dismiss" {
            self.utilities.open = false;
            self.utilities.options_open = false;
            self.utilities.print_popup = None;
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
            self.utilities.selection_only = id == "utilities.printSelection";
            self.utilities.print_popup = None;
            self.utilities.focus = 0;
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
        if transform.is_some() && (editor.read_only() || editor.busy()) {
            workspace.message = Some("The destination is read-only or has an edit pending".into());
            return true;
        }
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
        let destination = if export.is_some() {
            match self.platform.as_ref().map(|p| p.save_file()) {
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
            match bareline_platform_windows::printing::choose_printer() {
                Ok(Some(p)) => Some(p),
                Ok(None) => return true,
                Err(e) => {
                    workspace.message = Some(format!(
                        "Print unavailable: {e:?}. Try Print again to choose another printer."
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
        if algorithm.is_none() && transform.is_none() && export.is_none() && !printing && id != "utilities.statistics" {
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
        self.utilities.total = length;
        self.utilities.progress_seen = 0;
        self.utilities.open = true;
        self.utilities.options_open = false;
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
                    if let Some(algorithm) = algorithm {
                        let result = if let Some((source, _)) = paged {
                            let reader = core::PagedTextReader::new(source, range, cancel.clone())
                                .map_err(|e| format!("{e:?}"))?;
                            core::hash_reader(reader, algorithm, &cancel, |bytes| {
                                report(bytes as usize);
                                true
                            })
                        } else {
                            core::hash_snapshot(&snapshot, range, algorithm, &cancel)
                        }
                        .map_err(|e| format!("{e:?}"))?;
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
                                .map_err(|e| format!("{e:?}"))?
                                .read_to_string(&mut raw)
                                .map_err(|e| e.to_string())?;
                            let doc = bareline_document::Document::from_utf8(
                                &raw,
                                bareline_document::Budget::new(4 * 1024 * 1024),
                                bareline_document::Budget::new(0),
                            )
                            .map_err(|e| format!("{e:?}"))?;
                            let mut transaction = core::transform(
                                &doc.snapshot(),
                                TextOffset(0)..TextOffset(raw.len()),
                                kind,
                                1024 * 1024,
                                &cancel,
                            )
                            .map_err(|e| format!("{e:?}"))?;
                            transaction.base_revision = captured.revision;
                            for edit in &mut transaction.edits {
                                edit.range.start.0 += range.start.0;
                                edit.range.end.0 += range.start.0;
                            }
                            return Ok(UtilityResult::PagedEdit(captured, transaction));
                        }
                        let transaction = core::transform(&snapshot, range, kind, 16 * 1024 * 1024, &cancel)
                            .map_err(|e| format!("{e:?}"))?;
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
                            .map_err(|e| format!("{e:?}"))?;
                        let range = if print_selection {
                            range
                        } else {
                            TextOffset(0)..TextOffset(length)
                        };
                        let summary = if let Some((source, _)) = paged {
                            let reader =
                                core::PagedTextReader::new(source, TextOffset(0)..TextOffset(length), cancel.clone())
                                    .map_err(|e| format!("{e:?}"))?;
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
                                .map_err(|e| format!("{e:?}"))?
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
                                .map_err(|e| format!("{e:?}"))?;
                        core::statistics_reader(reader, revision, &cancel, &mut report)
                    } else {
                        core::statistics(&snapshot, 256 * 1024, &cancel, |bytes| {
                            report(bytes);
                            true
                        })
                    }
                    .map_err(|e| format!("{e:?}"))?;
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
                    Ok(()) => "Transformation queued as one undoable edit".into(),
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
                        Ok(()) => "Transformation queued as one undoable edit".into(),
                        Err(e) => format!("Transformation not applied: {e}"),
                    },
                    None => "Destination closed; transformation not applied".into(),
                }
            }
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
                    self.utilities_dispatch(el, id);
                } else if self.utilities.print_popup.is_some() {
                    self.utilities.print_popup = None;
                    self.utilities_redraw();
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        if self.utilities.print_popup.is_some() {
                            self.utilities.print_popup = None;
                        } else {
                            self.utilities.open = false;
                            self.utilities.options_open = false;
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
                    _ => {}
                }
                self.utilities_redraw();
                true
            }
            WindowEvent::MouseWheel { .. } | WindowEvent::Ime(_) => true,
            _ => false,
        }
    }
}
impl UtilitiesRuntime {
    pub(super) fn has_input_focus(&self) -> bool {
        self.open || self.options_open
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
            _ => "Result",
        }
    }
    /// Window coordinates; draw after editor operation translation and before palette.
    pub(super) fn draw(
        &mut self,
        settings: &settings::SettingsRuntime,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        use bareline_renderer::DrawOp;
        use bareline_ui::{rect, text};
        self.hits.clear();
        if !self.open {
            return;
        }
        let theme = settings.ui_theme();
        let w = (width - 32.0).clamp(280.0, 640.0);
        let h = if self.options_open { 460.0 } else { 300.0 };
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
            // Preview of the printed page composition (header and line numbers).
            let preview = rect(x + 24.0, y + 320.0, w - 48.0, 64.0);
            ops.push(DrawOp::Fill(preview, theme.editor));
            ops.push(DrawOp::StrokeRounded(preview, theme.border, 4.0, 1.0));
            text(ops, preview.x + 8.0, preview.y + 3.0, "Preview", 11.0, theme.muted);
            let mut py = preview.y + 20.0;
            if options.header && !options.title.is_empty() {
                text(ops, preview.x + 10.0, py, &options.title, 10.0, theme.muted);
                py += 14.0;
            }
            for (n, sample) in ["fn main() {", "    greet();"].iter().enumerate() {
                let line = if options.line_numbers {
                    format!("{:>3}  {sample}", n + 1)
                } else {
                    (*sample).into()
                };
                text(ops, preview.x + 10.0, py, &line, 11.0, theme.text);
                py += 14.0;
            }
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
        if !self.options_open {
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
            for line in value.lines() {
                let chars: Vec<char> = line.chars().collect();
                for part in chars.chunks(limit) {
                    if row >= 7 {
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
        write(&mut file).map_err(|e| format!("Export write content: {e:?}"))?;
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
        _ => panic!("unknown utilities accessibility fixture: {scenario}"),
    }
    let mut ops = Vec::new();
    shell.utilities.draw(&shell.settings, 1000.0, 800.0, &mut ops);
}

#[cfg(test)]
mod publication_tests {
    use super::*;
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
}
