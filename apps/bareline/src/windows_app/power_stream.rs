// SPDX-License-Identifier: MPL-2.0
//! Native admission/completion only; all source reading and staging runs off-thread.
use super::*;
use bareline_app::macros::{PowerReplayCompletion, PowerReplayRequest, PowerReplayTarget};
use bareline_app::workspace::WorkspaceEditor;
use bareline_document::{
    TextOffset,
    history::{EditMetadata, EditOrigin},
    paged::{PagedSnapshot, PreparedSourceTransaction},
};
use bareline_editor_surface::{
    TrackedEditReceipt,
    power::captured::{self, StagingOptions},
};
use bareline_file_io::cancellation::Cancellation;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone)]
enum Operation {
    Transform(String),
    Literal(String, Arguments),
    Input(Input),
    Clipboard(bool),
}
#[derive(Clone)]
struct Target {
    index: usize,
    secondary: bool,
    source: PagedSnapshot,
    selections: power::SelectionSet,
}
struct Promotion {
    index: usize,
    secondary: bool,
    identity: (u64, u64),
    id: String,
    arguments: Arguments,
    selections: power::SelectionSet,
}
enum Output {
    Prepared(PreparedSourceTransaction),
    Power(bareline_editor_surface::paged_power::PreparedPower),
    Rows(Vec<(usize, String)>),
    Clipboard(String),
}
struct Measurement {
    target: Target,
    operation: Operation,
    rows: Vec<(usize, String)>,
    next: usize,
    maps: std::collections::BTreeMap<usize, power::DisplayColumnMap>,
    font: String,
    pixels: f32,
    tabs: usize,
}
struct Worker {
    target: Target,
    operation: Operation,
    cancel: Cancellation,
    result: Receiver<Result<Output, String>>,
}
struct Replay {
    id: u64,
    index: usize,
    document: u64,
    cancelled: bool,
    resident: Option<TrackedEditReceipt>,
    terminal: Option<Result<(), String>>,
}
impl Replay {
    fn complete_once(&mut self, result: Result<(), String>) {
        if self.terminal.is_none() {
            self.terminal = Some(result);
        }
    }
}
impl PowerRuntime {
    fn stream_failed(&mut self, error: String) {
        if let Some(replay) = self.stream.replay.as_mut() {
            replay.complete_once(Err(error.clone()));
        }
        self.status = error;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Default)]
pub(super) struct StreamRuntime {
    worker: Option<Worker>,
    promotion: Option<Promotion>,
    receipt: Option<(Target, Operation, TrackedEditReceipt)>,
    replay: Option<Replay>,
    prepared: Option<bareline_editor_surface::paged_power::PreparedPower>,
    measurement: Option<Measurement>,
}

impl Shell {
    pub(super) fn power_paged_paste(&mut self) -> bool {
        let editor = if self.views.pane() == 1 {
            self.views.secondary.as_ref()
        } else {
            self.workspace
                .as_ref()
                .and_then(|workspace| workspace.editors.get(self.app.active))
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return false;
        };
        let rectangle = paged.capture_power().state.rectangle;
        let contents = match self
            .platform
            .as_ref()
            .ok_or("Clipboard unavailable")
            .and_then(|platform| {
                platform
                    .clipboard_text_with_metadata(power::consumer::RectangleClipboardMetadata::FORMAT, 262144)
                    .map_err(|_| "Clipboard unavailable")
            }) {
            Ok(contents) => contents,
            Err(error) => {
                self.power.stream_failed(error.into());
                return true;
            }
        };
        let metadata = contents
            .metadata
            .as_deref()
            .and_then(|bytes| power::consumer::RectangleClipboardMetadata::decode(bytes, &contents.text));
        let mut args = rectangle.map(rectangle_arguments).unwrap_or_default();
        args.insert("text".into(), contents.text);
        let id = if rectangle.is_some() {
            "editor.rectangle.paste"
        } else if let Some(metadata) = metadata {
            args.insert("rows".into(), metadata.row_widths.len().to_string());
            "editor.clipboard.rectangle"
        } else {
            "editor.paste.plainText"
        };
        self.power_paged_literal(id, args)
    }
    pub(super) fn power_paged_pointer(&mut self, event: &WindowEvent, pane: usize, point: Point) -> bool {
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let editor = if pane == 1 {
            self.views.secondary.as_ref()
        } else {
            workspace.editors.get(self.app.active)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return false;
        };
        let released = matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            }
        );
        if released && self.power.paged_rectangle_drag.is_none() {
            return false;
        }
        if !self.modifiers.alt_key() && !self.modifiers.control_key() && self.power.paged_rectangle_drag.is_none() {
            return false;
        }
        let hit = self
            .renderer
            .as_ref()
            .and_then(|renderer| paged.viewport().power_hit_position(renderer, point))
            .and_then(|(offset, _, _)| {
                paged.source_offset(
                    TextOffset(offset),
                    bareline_editor_surface::paged_view::SourceAffinity::After,
                )
            })
            .map(|offset| offset.0);
        let Some(offset) = hit else {
            if released {
                self.power.paged_rectangle_drag = None;
            }
            return true;
        };
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } if self.modifiers.alt_key() => {
                self.power.paged_rectangle_drag = Some((self.app.active, pane, paged.snapshot().clone(), offset));
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } if self.modifiers.control_key() => {
                let args = [("offset".into(), offset.to_string())].into_iter().collect();
                if let Err(error) = self.start_power_worker(
                    self.app.active,
                    pane == 1,
                    Operation::Literal("editor.caret.toggle".into(), args),
                    None,
                ) {
                    self.power.stream_failed(error);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } => {
                let Some((index, source_pane, source, anchor)) = self.power.paged_rectangle_drag.take() else {
                    return true;
                };
                if source_pane != pane || source.identity_token() != paged.snapshot().identity_token() {
                    self.power.stream_failed("Rectangle source changed".into());
                    return true;
                }
                let args = [
                    ("anchor_offset".into(), anchor.to_string()),
                    ("caret_offset".into(), offset.to_string()),
                ]
                .into_iter()
                .collect();
                if let Err(error) = self.start_power_worker(
                    index,
                    pane == 1,
                    Operation::Literal("editor.rectangle.gesture".into(), args),
                    None,
                ) {
                    self.power.stream_failed(error);
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn power_paged_literal(&mut self, id: &str, args: Arguments) -> bool {
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let secondary = self.views.pane() == 1;
        let editor = if secondary {
            self.views.secondary.as_ref()
        } else {
            workspace.editors.get(self.app.active)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return false;
        };
        if let Some(target) = self.power.global_target.as_ref() {
            if self.power.open && target.identity_token() != paged.snapshot().identity_token() {
                self.power.stream_failed("Dialog source changed; reopen it".into());
                return true;
            }
        }
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.promotion.is_some()
            || self.power.stream.receipt.is_some()
            || self.power.stream.replay.is_some()
        {
            self.power.status = "Power editing is busy".into();
            return true;
        }
        let result = bareline_editor_surface::paged_power::validate_arguments(id, &args).and_then(|()| {
            self.start_power_worker(self.app.active, secondary, Operation::Literal(id.into(), args), None)
        });
        if let Err(error) = result {
            self.power.stream_failed(error);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn power_paged_dispatch(&mut self, id: &str) -> bool {
        if !bareline_editor_surface::paged_power::supports_command(id) {
            return false;
        }
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let secondary = self.views.pane() == 1;
        let editor = if secondary {
            self.views.secondary.as_ref()
        } else {
            workspace.editors.get(self.app.active)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return false;
        };
        if id == "editor.column.insert" {
            return self.power_paged_literal("editor.column.context", Arguments::new());
        }
        if id == "editor.paste.fromHistory" {
            self.power.global_target = Some(paged.snapshot().clone());
            self.power.rectangle = paged.capture_power().state.rectangle;
            self.power.selected = 0;
            self.power.history_open = true;
            self.power.open = true;
            self.power.status = if self.power.history.enabled() {
                String::new()
            } else {
                "Enable Clipboard History to retain copied text.".into()
            };
            return true;
        }
        let mut args = Arguments::new();
        if id.starts_with("editor.rectangle.") {
            let Some(rectangle) = paged.capture_power().state.rectangle.or(self.power.rectangle) else {
                self.power.stream_failed("Select a rectangle first".into());
                return true;
            };
            args = rectangle_arguments(rectangle);
        }
        if matches!(id, "editor.paste.plainText" | "editor.rectangle.paste") {
            match self
                .platform
                .as_ref()
                .ok_or_else(|| "Clipboard unavailable".to_string())
                .and_then(|platform| {
                    platform
                        .clipboard_text_with_metadata(power::consumer::RectangleClipboardMetadata::FORMAT, 262144)
                        .map_err(|e| e.to_string())
                }) {
                Ok(contents) => {
                    let _verified = contents
                        .metadata
                        .as_deref()
                        .and_then(|bytes| power::consumer::RectangleClipboardMetadata::decode(bytes, &contents.text));
                    args.insert("text".into(), contents.text);
                }
                Err(error) => {
                    self.power.stream_failed(error);
                    return true;
                }
            }
        }
        self.power_paged_literal(id, args)
    }
    pub(super) fn power_stream_dispatch(&mut self, id: &str) -> bool {
        if power::transform_for_command(id).is_none() {
            return false;
        }
        if self.palette.open {
            return false;
        }
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.promotion.is_some()
            || self.power.stream.receipt.is_some()
            || self.power.stream.replay.is_some()
        {
            self.power.status = "A power edit is already pending.".into();
            return true;
        }
        self.power.status.clear();
        let secondary = self.views.pane() == 1;
        let Some(workspace) = self.workspace.as_mut() else {
            return false;
        };
        let selected = if secondary {
            self.views.secondary.as_mut()
        } else {
            workspace.editors.get_mut(self.app.active)
        };
        let Some(editor) = selected else {
            return false;
        };
        if editor.busy() || editor.read_only() {
            workspace.message = Some("Document is busy or read-only.".into());
            return true;
        }
        if let WorkspaceEditor::Resident(resident) = editor {
            match resident.execute_power_recorded(id, &Arguments::new()) {
                Ok(()) => return true,
                Err(error) if error.contains("BudgetExceeded") => {}
                Err(error) => {
                    resident.error = Some(error);
                    return true;
                }
            }
            let identity = resident.snapshot().identity_token();
            let selections = resident.selection_set();
            let index = workspace.editors.iter().position(
                |editor| matches!(editor,WorkspaceEditor::Resident(view) if view.snapshot().identity_token()==identity),
            );
            let Some(index) = index else {
                workspace.message = Some("The source tab must remain open for staging.".into());
                return true;
            };
            self.power.stream.promotion = Some(Promotion {
                index,
                secondary,
                identity,
                id: id.into(),
                arguments: Arguments::new(),
                selections,
            });
            (self.notify)();
            return true;
        }
        let index = self.app.active;
        if matches!(id, "editor.indent" | "editor.unindent") {
            if let WorkspaceEditor::Paged(paged) = editor {
                if let Some(rectangle) = paged.capture_power().state.rectangle {
                    return self.power_paged_literal(id, rectangle_arguments(rectangle));
                }
            }
        }
        if let Err(error) = self.start_power_worker(index, secondary, Operation::Transform(id.into()), None) {
            self.power.status = error;
        }
        true
    }
    pub(super) fn power_global_clipboard(&mut self, cut: bool) -> bool {
        let secondary = self.views.pane() == 1;
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let editor = if secondary {
            self.views.secondary.as_ref()
        } else {
            workspace.editors.get(self.app.active)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return false;
        };
        if let Some(rectangle) = paged.capture_power().state.rectangle {
            return self.power_paged_literal(
                if cut {
                    "editor.rectangle.cut"
                } else {
                    "editor.rectangle.copy"
                },
                rectangle_arguments(rectangle),
            );
        }
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.promotion.is_some()
            || self.power.stream.receipt.is_some()
            || self.power.stream.replay.is_some()
        {
            self.power.status = "A power edit is already pending.".into();
            return true;
        }
        if let Err(error) = self.start_power_worker(self.app.active, secondary, Operation::Clipboard(cut), None) {
            self.power.status = error;
        }
        true
    }
    fn start_power_worker(
        &mut self,
        index: usize,
        secondary: bool,
        operation: Operation,
        selections: Option<power::SelectionSet>,
    ) -> Result<(), String> {
        self.start_power_worker_mapped(index, secondary, operation, selections, None)
    }
    fn start_power_worker_mapped(
        &mut self,
        index: usize,
        secondary: bool,
        operation: Operation,
        selections: Option<power::SelectionSet>,
        maps: Option<std::collections::BTreeMap<usize, power::DisplayColumnMap>>,
    ) -> Result<(), String> {
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.receipt.is_some()
        {
            return Err("Power preparation is already pending".into());
        }
        let workspace = self.workspace.as_ref().ok_or("Workspace closed")?;
        let editor = if secondary {
            self.views.secondary.as_ref()
        } else {
            workspace.editors.get(index)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            return Err("Paged source is unavailable".into());
        };
        if paged.power_actor_busy()
            || paged.viewport().user_read_only && !matches!(operation, Operation::Clipboard(false))
        {
            return Err("Document is busy or read-only".into());
        }
        let selection = paged.global_selection();
        let captured = paged.read_handle();
        let global = paged.global_selection_set();
        let mut power_capture = paged.capture_power();
        power_capture.column_maps = maps;
        let target = Target {
            index,
            secondary,
            source: paged.snapshot().clone(),
            selections: global.clone(),
        };
        let selections = selections.or(Some(global));
        let ranges = selections
            .as_ref()
            .unwrap()
            .selections
            .iter()
            .map(|selection| {
                TextOffset(selection.anchor.min(selection.caret))..TextOffset(selection.anchor.max(selection.caret))
            })
            .collect::<Vec<_>>();
        if matches!(operation, Operation::Clipboard(_)) && ranges.iter().all(|range| range.is_empty()) {
            return Ok(());
        }
        let cancel = Cancellation::default();
        let options = StagingOptions {
            cache: std::env::temp_dir().join("Bareline-power-staging"),
            quota: workspace.transcode_quota_bytes,
            platform: std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
            source_options: bareline_file_io::source::SourceOptions::default(),
            budget: workspace.source_edit_budget(),
            memory: 16 << 20,
            cancellation: cancel.clone(),
        };
        let before = selections
            .map(|set| {
                set.selections
                    .into_iter()
                    .map(|selection| bareline_document::history::Selection {
                        anchor: TextOffset(selection.anchor),
                        caret: TextOffset(selection.caret),
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![bareline_document::history::Selection {
                    anchor: selection.0,
                    caret: selection.1,
                }]
            });
        let metadata = EditMetadata {
            before,
            origin: EditOrigin::Command,
            boundary: power::consumer::next_receipt_sequence(),
            ..Default::default()
        };
        let tab_width = paged.viewport().configured_tab_width();
        let work = operation.clone();
        let notify = self.notify.clone();
        let (send, result) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("power-staging".into())
            .spawn(move || {
                let outcome = match work {
                    Operation::Transform(id) => captured::prepare_transform(
                        captured,
                        &ranges,
                        power::transform_for_command(&id).expect("admitted transform"),
                        tab_width,
                        metadata,
                        &options,
                    )
                    .map(Output::Prepared),
                    Operation::Literal(id, args) => {
                        bareline_editor_surface::paged_power::measurement_rows(&power_capture, &id, &args, &options)
                            .and_then(|rows| {
                                if let Some(rows) = rows {
                                    Ok(Output::Rows(rows))
                                } else {
                                    bareline_editor_surface::paged_power::prepare(power_capture, &id, &args, &options)
                                        .map(Output::Power)
                                }
                            })
                            .map_err(std::io::Error::other)
                    }
                    Operation::Input(input) => bareline_editor_surface::paged_power::measurement_rows(
                        &power_capture,
                        "input",
                        &Arguments::new(),
                        &options,
                    )
                    .and_then(|rows| {
                        if let Some(rows) = rows {
                            Ok(Output::Rows(rows))
                        } else {
                            bareline_editor_surface::paged_power::prepare_input(power_capture, input, &options)
                                .map(Output::Power)
                        }
                    })
                    .map_err(std::io::Error::other),
                    Operation::Clipboard(cut) => captured::clipboard_text(
                        captured,
                        &ranges,
                        4 << 20,
                        options.budget.clone(),
                        options.cancellation.clone(),
                    )
                    .and_then(|text| {
                        if cut {
                            bareline_editor_surface::paged_power::prepare_input(
                                power_capture,
                                Input::Insert(String::new()),
                                &options,
                            )
                            .map(|mut prepared| {
                                prepared.clipboard = Some(text);
                                Output::Power(prepared)
                            })
                            .map_err(std::io::Error::other)
                        } else {
                            Ok(Output::Clipboard(text))
                        }
                    }),
                }
                .map_err(|error| error.to_string());
                let _ = send.send(outcome);
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.power.stream.worker = Some(Worker {
            target,
            operation,
            cancel,
            result,
        });
        self.power.status = "Preparing selected text…".into();
        Ok(())
    }
    pub(super) fn power_stream_pump(&mut self) -> bool {
        if let Some(mut job) = self.power.stream.measurement.take() {
            let editor = if job.target.secondary {
                self.views.secondary.as_ref()
            } else {
                self.workspace
                    .as_ref()
                    .and_then(|workspace| workspace.editors.get(job.target.index))
            };
            let Some(WorkspaceEditor::Paged(paged)) = editor else {
                self.power.stream_failed("Column target closed".into());
                return true;
            };
            if paged.snapshot().identity_token() != job.target.source.identity_token()
                || paged.global_selection_set() != job.target.selections
                || paged.viewport().font_family() != job.font
                || paged.viewport().configured_font_pixels() != job.pixels
                || paged.viewport().configured_tab_width() != job.tabs
            {
                self.power
                    .stream_failed("Column source, selection or font changed".into());
                return true;
            }
            let Some(renderer) = self.renderer.as_mut() else {
                self.power.stream_failed("Column renderer unavailable".into());
                return true;
            };
            let stop = job.next.saturating_add(8).min(job.rows.len());
            for (number, text) in &job.rows[job.next..stop] {
                match bareline_editor_surface::measure_column_text(
                    renderer,
                    text.trim_end_matches(['\r', '\n']),
                    job.pixels,
                    &job.font,
                    job.tabs,
                ) {
                    Ok(map) => {
                        job.maps.insert(*number, map);
                    }
                    Err(error) => {
                        self.power.stream_failed(error);
                        return true;
                    }
                }
            }
            job.next = stop;
            if stop < job.rows.len() {
                self.power.stream.measurement = Some(job);
                (self.notify)();
            } else if let Err(error) = self.start_power_worker_mapped(
                job.target.index,
                job.target.secondary,
                job.operation,
                Some(job.target.selections),
                Some(job.maps),
            ) {
                self.power.stream_failed(error);
            }
            return true;
        }
        if let Some(workspace) = self.workspace.as_mut() {
            for editor in &mut workspace.editors {
                if let WorkspaceEditor::Paged(paged) = editor {
                    paged.enable_power_input();
                }
            }
            if let Some(WorkspaceEditor::Paged(paged)) = self.views.secondary.as_mut() {
                paged.enable_power_input();
            }
        }
        if self.power.stream.worker.is_none()
            && self.power.stream.measurement.is_none()
            && self.power.stream.promotion.is_none()
            && self.power.stream.receipt.is_none()
            && self.power.stream.replay.is_none()
        {
            let next = if let Some(workspace) = self.workspace.as_mut() {
                let primary = workspace.editors.iter_mut().enumerate().find_map(|(index, editor)| {
                    if let WorkspaceEditor::Paged(paged) = editor {
                        paged.take_power_input().map(|input| (index, false, input))
                    } else {
                        None
                    }
                });
                primary.or_else(|| {
                    self.views.secondary.as_mut().and_then(|editor| {
                        if let WorkspaceEditor::Paged(paged) = editor {
                            paged.take_power_input().map(|input| (self.app.active, true, input))
                        } else {
                            None
                        }
                    })
                })
            } else {
                None
            };
            if let Some((index, secondary, input)) = next {
                if let Err(error) = self.start_power_worker(index, secondary, Operation::Input(input), None) {
                    if let Some(workspace) = self.workspace.as_mut() {
                        let editor = if secondary {
                            self.views.secondary.as_mut()
                        } else {
                            workspace.editors.get_mut(index)
                        };
                        if let Some(WorkspaceEditor::Paged(paged)) = editor {
                            paged.finish_power_preparation();
                        }
                    }
                    self.power.stream_failed(error);
                }
                return true;
            }
            let refresh = if let Some(workspace) = self.workspace.as_mut() {
                workspace
                    .editors
                    .iter_mut()
                    .enumerate()
                    .find_map(|(index, editor)| {
                        if let WorkspaceEditor::Paged(paged) = editor {
                            paged.take_power_hidden_refresh().then_some((index, false))
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        self.views.secondary.as_mut().and_then(|editor| {
                            if let WorkspaceEditor::Paged(paged) = editor {
                                paged.take_power_hidden_refresh().then_some((self.app.active, true))
                            } else {
                                None
                            }
                        })
                    })
            } else {
                None
            };
            if let Some((index, secondary)) = refresh {
                if let Err(error) = self.start_power_worker(
                    index,
                    secondary,
                    Operation::Literal("editor.lines.refreshHidden".into(), Arguments::new()),
                    None,
                ) {
                    self.power.stream_failed(error);
                }
                return true;
            }
        }
        if let Some(promotion) = self.power.stream.promotion.take() {
            let Some(workspace) = self.workspace.as_mut() else {
                self.power.stream_failed("Workspace closed during preparation".into());
                return true;
            };
            let current = if promotion.secondary {
                self.views.secondary.as_ref()
            } else {
                workspace.editors.get(promotion.index)
            };
            if matches!(current,Some(WorkspaceEditor::Resident(view)) if view.selection_set()!=promotion.selections) {
                self.power
                    .stream_failed("Selection changed during staging promotion.".into());
                return true;
            }
            match workspace.promote_resident_for_source_edit(promotion.index, promotion.identity) {
                Ok(false) => {
                    self.power.stream.promotion = Some(promotion);
                    return false;
                }
                Err(error) => {
                    self.power.stream_failed(error);
                    return true;
                }
                Ok(true) => {
                    if promotion.secondary {
                        let current = self.views.secondary.as_ref();
                        if !matches!(current,Some(WorkspaceEditor::Paged(view)) if view.snapshot().identity_token()==promotion.identity)
                        {
                            if !matches!(current,Some(WorkspaceEditor::Resident(view)) if view.snapshot().identity_token()==promotion.identity&&view.selection_set()==promotion.selections)
                            {
                                self.power
                                    .stream_failed("Selection changed during staging promotion.".into());
                                return true;
                            }
                            let Some(WorkspaceEditor::Paged(source)) = workspace.editors.get(promotion.index) else {
                                self.power.stream_failed("Promoted source unavailable".into());
                                return true;
                            };
                            match source.clone_view() {
                                Ok(mut clone) => {
                                    if let Some(current) = current {
                                        current.copy_presentation_to(clone.viewport_mut());
                                    }
                                    if let Some(old) = self.views.secondary.replace(WorkspaceEditor::Paged(clone)) {
                                        self.views.retired.push(old);
                                    }
                                }
                                Err(error) => {
                                    self.power.stream_failed(error);
                                    return true;
                                }
                            }
                        }
                    }
                    let target = if promotion.secondary {
                        self.views.secondary.as_ref()
                    } else {
                        workspace.editors.get(promotion.index)
                    };
                    if target.is_some_and(|editor| editor.busy()) {
                        self.power.stream.promotion = Some(promotion);
                        return false;
                    }
                    let primary = promotion.selections.primary();
                    if matches!(target,Some(WorkspaceEditor::Paged(view)) if view.global_selection()!=(TextOffset(primary.anchor),TextOffset(primary.caret)))
                    {
                        self.power
                            .stream_failed("Selection changed during staging promotion.".into());
                        return true;
                    }
                    let operation =
                        if power::transform_for_command(&promotion.id).is_some() && promotion.arguments.is_empty() {
                            Operation::Transform(promotion.id)
                        } else {
                            Operation::Literal(promotion.id, promotion.arguments)
                        };
                    if let Err(error) = self.start_power_worker(
                        promotion.index,
                        promotion.secondary,
                        operation,
                        Some(promotion.selections),
                    ) {
                        self.power.stream_failed(error);
                    }
                    return true;
                }
            }
        }
        if let Some((target, operation, receipt)) = self.power.stream.receipt.take() {
            match receipt.terminal() {
                None => {
                    self.power.stream.receipt = Some((target, operation, receipt));
                }
                Some(Err(error)) => {
                    self.power.stream.prepared = None;
                    self.power.stream_failed(error);
                    return true;
                }
                Some(Ok(revision)) => {
                    if let Some(replay) = self.power.stream.replay.as_mut() {
                        replay.complete_once(Ok(()));
                    }
                    if let Some(workspace) = self.workspace.as_ref() {
                        let editor = if target.secondary {
                            self.views.secondary.as_ref()
                        } else {
                            workspace.editors.get(target.index)
                        };
                        if editor.is_some_and(|editor| match editor {
                            WorkspaceEditor::Paged(paged) => paged.power_actor_busy(),
                            WorkspaceEditor::Resident(view) => view.busy(),
                        }) {
                            self.power.stream.receipt = Some((target, operation, receipt));
                            return false;
                        }
                    }
                    if let Some(prepared) = self.power.stream.prepared.take() {
                        if let Some(workspace) = self.workspace.as_mut() {
                            let editor = if target.secondary {
                                self.views.secondary.as_mut()
                            } else {
                                workspace.editors.get_mut(target.index)
                            };
                            if let Some(WorkspaceEditor::Paged(paged)) = editor {
                                if let Err(error) = paged.install_power_state(
                                    &prepared.source,
                                    revision,
                                    prepared.selections,
                                    prepared.state,
                                    &prepared.hidden_lines,
                                ) {
                                    self.power.stream_failed(error);
                                    return true;
                                }
                            }
                        }
                    }
                    if let Operation::Input(input) = &operation {
                        if let Some(workspace) = self.workspace.as_mut() {
                            let editor = if target.secondary {
                                self.views.secondary.as_mut()
                            } else {
                                workspace.editors.get_mut(target.index)
                            };
                            if let Some(WorkspaceEditor::Paged(paged)) = editor {
                                if let Err(error) =
                                    paged.acknowledge_power_input(Some(&receipt), &target.source, input.clone())
                                {
                                    self.power.stream_failed(error);
                                    return true;
                                }
                            }
                        }
                    }
                    if self.power.stream.replay.is_none()
                        && matches!(operation, Operation::Transform(_) | Operation::Literal(..))
                    {
                        let (id, args) = match operation {
                            Operation::Transform(id) => (id, Arguments::new()),
                            Operation::Literal(id, args) => (id, args),
                            _ => unreachable!(),
                        };
                        if let Some(workspace) = self.workspace.as_mut() {
                            let editor = if target.secondary {
                                self.views.secondary.as_mut()
                            } else {
                                workspace.editors.get_mut(target.index)
                            };
                            if let Some(WorkspaceEditor::Paged(paged)) = editor {
                                if let Err(error) = paged.acknowledge_tracked_power(&receipt, &id, &args) {
                                    self.power.status = error;
                                    return true;
                                }
                            }
                        }
                    }
                    self.power.status.clear();
                    return true;
                }
            }
        }
        let Some(worker) = self.power.stream.worker.take() else {
            return false;
        };
        let Some(workspace) = self.workspace.as_mut() else {
            self.power.stream_failed("Workspace closed during preparation".into());
            return true;
        };
        let editor = if worker.target.secondary {
            self.views.secondary.as_mut()
        } else {
            workspace.editors.get_mut(worker.target.index)
        };
        let Some(WorkspaceEditor::Paged(paged)) = editor else {
            self.power.stream_failed("Source view closed during staging.".into());
            return true;
        };
        if paged.snapshot().identity_token() != worker.target.source.identity_token()
            || paged.snapshot().content_state != worker.target.source.content_state
            || paged.global_selection_set() != worker.target.selections
        {
            paged.finish_power_preparation();
            self.power
                .stream_failed("Document or selection changed; power edit cancelled.".into());
            return true;
        }
        let outcome = match worker.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => {
                self.power.stream.worker = Some(worker);
                return false;
            }
            Err(TryRecvError::Disconnected) => Err("Power worker stopped".into()),
        };
        paged.finish_power_preparation();
        match outcome {
            Err(error) => self.power.stream_failed(error),
            Ok(Output::Rows(rows)) => {
                self.power.stream.measurement = Some(Measurement {
                    target: worker.target.clone(),
                    operation: worker.operation.clone(),
                    rows,
                    next: 0,
                    maps: Default::default(),
                    font: paged.viewport().font_family().into(),
                    pixels: paged.viewport().configured_font_pixels(),
                    tabs: paged.viewport().configured_tab_width(),
                });
                (self.notify)();
            }
            Ok(Output::Prepared(prepared)) => {
                match paged.apply_prepared_source_tracked(&worker.target.source, prepared) {
                    Ok(receipt) => {
                        self.power.stream.receipt = Some((worker.target.clone(), worker.operation.clone(), receipt));
                        self.power.status = "Applying selected transform…".into();
                    }
                    Err(error) => self.power.stream_failed(error),
                }
            }
            Ok(Output::Power(mut prepared)) => {
                if self.power.stream.replay.is_none() {
                    if let Some(text) = prepared.clipboard.as_ref() {
                        let metadata = prepared.clipboard_rectangle.and_then(|rectangle| {
                            power::consumer::RectangleClipboardMetadata {
                                row_widths: vec![
                                    (rectangle.end_column - rectangle.start_column) as u32;
                                    rectangle.last_line - rectangle.first_line + 1
                                ],
                            }
                            .encode(text)
                            .ok()
                        });
                        if let Err(error) = self
                            .platform
                            .as_ref()
                            .ok_or_else(|| "Clipboard unavailable".to_string())
                            .and_then(|platform| {
                                if let Some(metadata) = metadata {
                                    platform
                                        .set_clipboard_text_with_metadata(
                                            text,
                                            power::consumer::RectangleClipboardMetadata::FORMAT,
                                            &metadata,
                                        )
                                        .map_err(|e| e.to_string())
                                } else {
                                    platform.set_clipboard_text(text).map_err(|e| e.to_string())
                                }
                            })
                        {
                            self.power.stream_failed(error);
                            return true;
                        }
                        self.power.copied(text);
                    }
                }
                let operation = match &worker.operation {
                    Operation::Literal(id, _) => Operation::Literal(
                        if matches!(id.as_str(), "editor.rectangle.gesture" | "editor.rectangle.extend") {
                            "editor.rectangle.select".into()
                        } else if id == "editor.clipboard.rectangle" {
                            "editor.rectangle.paste".into()
                        } else {
                            id.clone()
                        },
                        prepared.arguments.clone(),
                    ),
                    Operation::Input(input) => Operation::Input(input.clone()),
                    Operation::Clipboard(true) => Operation::Input(Input::Insert(String::new())),
                    _ => unreachable!(),
                };
                if matches!(&operation,Operation::Literal(id,_) if id=="editor.column.context") {
                    self.power.rectangle = prepared.state.rectangle;
                    self.power.global_target = Some(prepared.source.clone());
                    self.power.open = true;
                    self.power.history_open = false;
                    self.power.focus = 0;
                }
                if let Some(transaction) = prepared.transaction.take() {
                    match paged.apply_prepared_source_tracked(&prepared.source, transaction) {
                        Ok(receipt) => {
                            self.power.stream.receipt = Some((worker.target.clone(), operation, receipt));
                            self.power.stream.prepared = Some(prepared);
                        }
                        Err(error) => self.power.stream_failed(error),
                    }
                } else {
                    match paged.install_power_state(
                        &prepared.source,
                        prepared.source.revision,
                        prepared.selections,
                        prepared.state,
                        &prepared.hidden_lines,
                    ) {
                        Ok(()) => {
                            if let Some(replay) = self.power.stream.replay.as_mut() {
                                replay.complete_once(Ok(()));
                            } else {
                                let result = match operation {
                                    Operation::Literal(id, _)
                                        if matches!(
                                            id.as_str(),
                                            "editor.column.context" | "editor.lines.refreshHidden"
                                        ) =>
                                    {
                                        Ok(())
                                    }
                                    Operation::Literal(id, args) => {
                                        paged.acknowledge_power_view(&prepared.source, &id, &args)
                                    }
                                    Operation::Input(input) => {
                                        paged.acknowledge_power_input(None, &prepared.source, input)
                                    }
                                    _ => unreachable!(),
                                };
                                if let Err(error) = result {
                                    self.power.stream_failed(error);
                                    return true;
                                }
                            }
                            self.power.status.clear();
                        }
                        Err(error) => self.power.stream_failed(error),
                    }
                }
            }
            Ok(Output::Clipboard(text)) => {
                let result = self
                    .platform
                    .as_ref()
                    .ok_or_else(|| "Clipboard unavailable".to_string())
                    .and_then(|platform| platform.set_clipboard_text(&text).map_err(|error| error.to_string()));
                match result {
                    Err(error) => self.power.status = error,
                    Ok(()) => {
                        self.power.copied(&text);
                        if matches!(worker.operation, Operation::Clipboard(true)) {
                            let transaction = bareline_document::EditTransaction {
                                base_revision: worker.target.source.revision,
                                edits: worker
                                    .target
                                    .selections
                                    .selections
                                    .iter()
                                    .map(|selection| bareline_document::Edit {
                                        range: TextOffset(selection.anchor.min(selection.caret))
                                            ..TextOffset(selection.anchor.max(selection.caret)),
                                        insert: String::new(),
                                    })
                                    .collect(),
                            };
                            match paged.apply_prepared_tracked(&worker.target.source, transaction) {
                                Ok(receipt) => {
                                    self.power.stream.receipt =
                                        Some((worker.target.clone(), worker.operation.clone(), receipt))
                                }
                                Err(error) => self.power.status = error,
                            }
                        }
                    }
                }
            }
        }
        true
    }
    pub(super) fn power_stream_cancel(&mut self) -> bool {
        if self.power.stream.worker.is_none()
            && self.power.stream.measurement.is_none()
            && self.power.stream.promotion.is_none()
        {
            return false;
        }
        if let Some(worker) = self.power.stream.worker.as_ref() {
            if let Some(workspace) = self.workspace.as_mut() {
                let editor = if worker.target.secondary {
                    self.views.secondary.as_mut()
                } else {
                    workspace.editors.get_mut(worker.target.index)
                };
                if let Some(WorkspaceEditor::Paged(paged)) = editor {
                    paged.finish_power_preparation();
                }
            }
        }
        self.power.stream.worker = None;
        self.power.stream.measurement = None;
        self.power.stream.promotion = None;
        self.power.stream_failed("Power preparation cancelled.".into());
        if let Some(workspace) = self.workspace.as_mut() {
            workspace.message = Some(self.power.status.clone());
        }
        true
    }
}

impl Shell {
    pub(crate) fn power_replay_start(&mut self, request: PowerReplayRequest) -> Result<(), String> {
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.promotion.is_some()
            || self.power.stream.receipt.is_some()
            || self.power.stream.replay.is_some()
        {
            return Err("Power editing is busy".into());
        }
        bareline_editor_surface::paged_power::validate_arguments(&request.command, &request.arguments)?;
        let workspace = self.workspace.as_mut().ok_or("Workspace closed")?;
        let editor = workspace
            .editors
            .get_mut(request.target_index)
            .ok_or("Replay target closed")?;
        let document = match (&request.target, &*editor) {
            (PowerReplayTarget::Resident(source), WorkspaceEditor::Resident(view))
                if source.identity_token() == view.snapshot().identity_token()
                    && source.content_state == view.snapshot().content_state =>
            {
                source.identity_token().0
            }
            (PowerReplayTarget::Paged(source), WorkspaceEditor::Paged(view))
                if source.identity_token() == view.snapshot().identity_token()
                    && source.content_state == view.snapshot().content_state =>
            {
                source.identity_token().0
            }
            _ => return Err("Replay target changed before admission".into()),
        };
        if editor.busy() || editor.read_only() {
            return Err("Replay target is busy or read-only".into());
        }
        let selections = match &*editor {
            WorkspaceEditor::Resident(view) => view.selection_set(),
            WorkspaceEditor::Paged(view) => view.global_selection_set(),
        };
        if selections != request.selections {
            return Err("Replay selection changed before admission".into());
        }
        self.power.status.clear();
        let mut replay = Replay {
            id: request.id,
            index: request.target_index,
            document,
            cancelled: false,
            resident: None,
            terminal: None,
        };
        if let WorkspaceEditor::Resident(view) = editor {
            match view.execute_power_tracked(&request.command, &request.arguments) {
                Ok(receipt) => {
                    replay.resident = Some(receipt);
                    self.power.stream.replay = Some(replay);
                    return Ok(());
                }
                Err(error) if error.contains("BudgetExceeded") => {
                    self.power.stream.promotion = Some(Promotion {
                        index: request.target_index,
                        secondary: false,
                        identity: view.snapshot().identity_token(),
                        id: request.command,
                        arguments: request.arguments,
                        selections: request.selections,
                    });
                    self.power.stream.replay = Some(replay);
                    (self.notify)();
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
        let operation = if power::transform_for_command(&request.command).is_some() && request.arguments.is_empty() {
            Operation::Transform(request.command)
        } else {
            Operation::Literal(request.command, request.arguments)
        };
        self.start_power_worker(request.target_index, false, operation, Some(request.selections))?;
        self.power.stream.replay = Some(replay);
        Ok(())
    }
    pub(crate) fn power_replay_poll(&mut self, id: u64) -> Option<PowerReplayCompletion> {
        let replay = self.power.stream.replay.as_ref()?;
        if replay.id != id {
            return None;
        }
        let resident_result = match replay.resident.as_ref() {
            Some(receipt) => Some(receipt.terminal()?),
            None => None,
        };
        if self.power.stream.worker.is_some()
            || self.power.stream.measurement.is_some()
            || self.power.stream.promotion.is_some()
            || self.power.stream.receipt.is_some()
        {
            return None;
        }
        let current = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(replay.index));
        if current.is_some_and(|editor| editor.busy()) {
            return None;
        }
        let target = current.and_then(|editor| match editor {
            WorkspaceEditor::Resident(view) if view.snapshot().identity_token().0 == replay.document => {
                Some(PowerReplayTarget::Resident(view.snapshot().clone()))
            }
            WorkspaceEditor::Paged(view) if view.snapshot().identity_token().0 == replay.document => {
                Some(PowerReplayTarget::Paged(view.snapshot().clone()))
            }
            _ => None,
        });
        let result = if let Some(result) = resident_result {
            result.map(|_| ())
        } else if let Some(result) = replay.terminal.clone() {
            result
        } else if replay.cancelled {
            Err("Replay power operation cancelled".into())
        } else {
            return None;
        };
        self.power.stream.replay = None;
        Some(PowerReplayCompletion { target, result })
    }
    pub(crate) fn power_replay_cancel(&mut self, id: u64) {
        let Some(replay) = self.power.stream.replay.as_mut() else {
            return;
        };
        if replay.id != id {
            return;
        }
        replay.cancelled = true;
        if let Some(worker) = self.power.stream.worker.as_ref() {
            let editor = if worker.target.secondary {
                self.views.secondary.as_mut()
            } else {
                self.workspace
                    .as_mut()
                    .and_then(|workspace| workspace.editors.get_mut(worker.target.index))
            };
            if let Some(WorkspaceEditor::Paged(paged)) = editor {
                paged.finish_power_preparation();
            }
        }
        self.power.stream.worker = None;
        self.power.stream.measurement = None;
        self.power.stream.promotion = None;
        // An admitted actor mutation cannot be withdrawn; retain its receipt
        // until publication/failure before reporting the final target identity.
    }
}

#[cfg(test)]
mod replay_terminal_tests {
    use super::*;
    #[test]
    fn unrelated_status_cannot_turn_committed_replay_into_retryable_failure() {
        let mut runtime = PowerRuntime::default();
        runtime.stream.replay = Some(Replay {
            id: 1,
            index: 0,
            document: 9,
            cancelled: false,
            resident: None,
            terminal: None,
        });
        runtime.status = "A different control is unavailable".into();
        assert!(runtime.stream.replay.as_ref().unwrap().terminal.is_none());
        runtime.stream.replay.as_mut().unwrap().complete_once(Ok(()));
        runtime.status = "A power edit is already pending".into();
        runtime.stream_failed("Late failure from another UI action".into());
        assert_eq!(runtime.stream.replay.as_ref().unwrap().terminal, Some(Ok(())));
        let delivered = runtime.stream.replay.take().unwrap().terminal;
        assert_eq!(delivered, Some(Ok(())));
        assert!(runtime.stream.replay.is_none());
    }
    #[test]
    fn failed_preparation_stays_failed_when_status_is_cleared() {
        let mut runtime = PowerRuntime::default();
        runtime.stream.replay = Some(Replay {
            id: 1,
            index: 0,
            document: 9,
            cancelled: false,
            resident: None,
            terminal: None,
        });
        runtime.stream_failed("Staging quota exceeded".into());
        runtime.status.clear();
        assert_eq!(
            runtime.stream.replay.as_ref().unwrap().terminal,
            Some(Err("Staging quota exceeded".into()))
        );
    }
}
