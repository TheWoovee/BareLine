// SPDX-License-Identifier: MPL-2.0
//! Explicit generated-fixture workload driver; absent in ordinary launches.
use super::*;
use std::ffi::OsString;

#[derive(Clone)]
pub(super) struct PerformanceConfig {
    pub root: PathBuf,
    pub input: Option<PathBuf>,
    workload: String,
    origin_ns: Option<u128>,
    extension_command: Option<String>,
}
impl PerformanceConfig { pub(super) fn requires_extensions(&self) -> bool { self.workload == "extensions-memory" } }

pub(super) fn parse_args(args: &[OsString]) -> Result<(Vec<OsString>, Option<PerformanceConfig>), Box<dyn std::error::Error>> {
    let (mut filtered, mut workload, mut root, mut input, mut origin_ns, mut extension_command) = (Vec::new(), None, None, None, None, None);
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" { filtered.extend_from_slice(&args[index..]); break; }
        let slot = match args[index].to_str() {
            Some("--perf-workload") => Some(0), Some("--perf-root") => Some(1),
            Some("--perf-input") => Some(2), Some("--perf-origin-ns") => Some(3), Some("--perf-extension-command") => Some(4), _ => None,
        };
        if let Some(slot) = slot {
            index += 1;
            let value = args.get(index).ok_or("Missing performance option value")?.clone();
            let target = match slot { 0 => &mut workload, 1 => &mut root, 2 => &mut input, 3 => &mut origin_ns, _ => &mut extension_command };
            if target.replace(value).is_some() { return Err("Duplicate performance option".into()); }
        } else { filtered.push(args[index].clone()); }
        index += 1;
    }
    let Some(workload) = workload else {
        if root.is_some() || input.is_some() || origin_ns.is_some() || extension_command.is_some() { return Err("Performance paths require --perf-workload".into()); }
        return Ok((filtered, None));
    };
    let workload = workload.into_string().map_err(|_| "Invalid workload")?;
    if !["launch", "idle", "open", "scroll", "edit", "syntax", "literal-search", "regex-search", "search-cancel", "result-jump", "save", "save-as", "workspace-scan", "tail-append", "extensions-memory", "tabs-100", "tabs-500"].contains(&workload.as_str()) {
        return Err("Unknown performance workload".into());
    }
    let root = PathBuf::from(root.ok_or("--perf-root is required")?).canonicalize()?;
    let marker = std::fs::symlink_metadata(root.join(".bareline-perf"))?;
    if !marker.is_file() || marker.file_type().is_symlink() || marker.len() != 0 {
        return Err("Performance root requires an empty regular .bareline-perf marker".into());
    }
    let input = input.map(PathBuf::from).map(|p| p.canonicalize()).transpose()?;
    if input.as_ref().is_some_and(|p| !p.starts_with(&root) || !p.is_file()) {
        return Err("Performance input must be a file within the generated root".into());
    }
    if !workload.starts_with("tabs-") && !["launch", "idle"].contains(&workload.as_str()) && input.is_none() { return Err("Workload requires --perf-input".into()); }
    if filtered.iter().any(|a| a == "--perf" || a == "--smoke" || a == "--text-prototype") {
        return Err("Performance workload cannot be combined with legacy diagnostic modes".into());
    }
    let origin_ns = origin_ns.map(|value| value.to_str().ok_or("Invalid performance clock origin")?.parse::<u128>().map_err(|_| "Invalid performance clock origin")).transpose()?;
    if origin_ns.is_some_and(|origin| clock_ns().is_none_or(|now| origin > now || now - origin > 120_000_000_000)) { return Err("Performance clock origin is outside the current bounded run".into()); }
    let extension_command = extension_command.map(|value| value.into_string().map_err(|_| "Invalid extension command")).transpose()?;
    if (workload == "extensions-memory") != extension_command.is_some() || extension_command.as_ref().is_some_and(|value| value.len() > 512 || !value.contains('/')) { return Err("Extension workload requires an explicit owner/command identity".into()); }
    Ok((filtered, Some(PerformanceConfig { root, input, workload, origin_ns, extension_command })))
}

fn clock_ns() -> Option<u128> {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn QueryPerformanceCounter(value: *mut i64) -> i32; fn QueryPerformanceFrequency(value: *mut i64) -> i32; }
    let (mut value, mut frequency) = (0i64, 0i64);
    // QPC is the same process-independent clock used by Python perf_counter_ns
    // on supported Windows. Fail closed instead of mixing wall clocks.
    if unsafe { QueryPerformanceCounter(&mut value) == 0 || QueryPerformanceFrequency(&mut frequency) == 0 } || value < 0 || frequency <= 0 { return None; }
    Some(value as u128 * 1_000_000_000 / frequency as u128)
}

#[derive(Default)]
pub(super) struct PerformanceRuntime {
    config: Option<PerformanceConfig>,
    started: Option<Instant>,
    operation: Option<Instant>,
    phase: u8,
    frame_ready: bool,
    samples: Vec<u128>,
    first_frame_us: Option<u128>,
    editable_us: Option<u128>,
    output: Option<PathBuf>,
    process_present_us: Option<u128>,
    source_ready_us: Option<u128>,
    full_load_us: Option<u128>,
    extra: Vec<(&'static str, u128)>,
    append: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    expected_tail: Option<usize>,
    frame_tail_len: Option<usize>,
    syntax_present: bool,
    syntax_present_us: Option<u128>,
    scroll_before: Option<(usize, u64)>,
    frame_scroll: Option<(usize, u64)>,
    cancelled_search: Option<bareline_search::SearchJob>,
    extension_generation: Option<u64>,
}
impl PerformanceRuntime {
    pub(super) fn configure(&mut self, config: Option<PerformanceConfig>) {
        self.config = config;
        self.started = self.config.as_ref().map(|_| Instant::now());
    }
    pub(super) fn enabled(&self) -> bool { self.config.is_some() }
    fn finish(&mut self, success: bool) {
        if success {
            let mut metrics = Vec::new();
            if let Some(value) = self.first_frame_us { metrics.push(format!("\"driver_first_frame_us\":{value}")); }
            if let Some(value) = self.editable_us { metrics.push(format!("\"editable_viewport_us\":{value}")); }
            if let Some(value) = self.process_present_us { metrics.push(format!("\"process_launch_to_first_present_us\":{value}")); }
            if let Some(value) = self.source_ready_us { metrics.push(format!("\"source_ready_us\":{value}")); }
            if let Some(value) = self.full_load_us { metrics.push(format!("\"resident_full_load_us\":{value}")); }
            for (name, value) in &self.extra { metrics.push(format!("\"{name}\":{value}")); }
            if let Some(operation) = self.operation { metrics.push(format!("\"operation_us\":{}", operation.elapsed().as_micros())); }
            if let Ok(value) = bareline_platform_windows::private_bytes() { metrics.push(format!("\"private_bytes_point\":{value}")); }
            if !self.samples.is_empty() {
                let mut ordered = self.samples.clone(); ordered.sort_unstable();
                metrics.push(format!("\"scroll_step_to_present_p50_us\":{},\"scroll_step_to_present_p95_us\":{},\"scroll_samples\":{},\"scroll_stalls_over_100ms\":{}",
                    ordered[(ordered.len()-1)/2], ordered[(ordered.len()*95).div_ceil(100)-1], ordered.len(), ordered.iter().filter(|v| **v > 100_000).count()));
            }
            let receipt = format!("{{\"event\":\"measurement\",\"metrics\":{{{}}}}}", metrics.join(","));
            // The external owned-Job adapter reads this create-new receipt after
            // process exit. No inherited console/pipe handle is necessary.
            let written = self.config.as_ref().is_some_and(|config| {
                std::fs::OpenOptions::new().write(true).create_new(true)
                    .open(config.root.join("performance-result.json"))
                    .and_then(|mut file| std::io::Write::write_all(&mut file, receipt.as_bytes())).is_ok()
            });
            if written { println!("{receipt}"); }
            else { eprintln!("Performance receipt could not be created; no successful measurement"); }
        } else {
            eprintln!("Performance workload failed or timed out; no success measurement emitted");
        }
        self.phase = 255;
    }
    fn pump(&mut self, workspace: &mut Workspace, app: &mut App) -> bool {
        let Some(config) = self.config.clone() else { return false; };
        if self.phase == 255 { return true; }
        if self.started.is_some_and(|start| start.elapsed() > Duration::from_secs(120)) { self.finish(false); return true; }
        if self.phase == 0 {
            if let Some(input) = &config.input {
                if config.workload == "tail-append" { workspace.resident_max_bytes = 0; }
                workspace.open(input.clone());
            }
            else {
                if workspace.new_document().is_err() { self.finish(false); return true; }
                if config.workload.starts_with("tabs-") { workspace.editors[0].enqueue(Input::Insert("PERF_NEEDLE populated document\n".into())); }
            }
            self.frame_ready = false;
            self.phase = 1;
            return false;
        }
        if workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) { return false; }
        if workspace.editors.is_empty() || workspace.editors.iter().any(|editor| editor.error.is_some()) { self.finish(false); return true; }
        app.tabs = workspace.titles();
        app.active = app.active.min(workspace.editors.len() - 1);
        if self.source_ready_us.is_none() {
            self.source_ready_us = self.started.map(|start| start.elapsed().as_micros());
            if config.input.is_some() && !workspace.editors[app.active].paged() { self.full_load_us = self.source_ready_us; }
        }
        if self.phase == 1 {
            if !self.frame_ready { return false; }
            self.frame_ready = false;
            self.editable_us = self.started.map(|start| start.elapsed().as_micros());
            self.operation = Some(Instant::now());
            match config.workload.as_str() {
                "launch" | "open" => { self.finish(true); return true; }
                "edit" => workspace.editors[app.active].enqueue(Input::Insert("PERF_EDIT".into())),
                "literal-search" | "regex-search" | "search-cancel" | "result-jump" => {
                    workspace.find.show(); workspace.find.field.insert(if config.workload == "result-jump" { "PERF_NEEDLE" } else { "PERF_ABSENT_TOKEN" });
                    if config.workload == "regex-search" { workspace.find.toggle_mode(); workspace.find.toggle_mode(); }
                }
                "scroll" => {
                    self.scroll_before = scroll_position(&workspace.editors[app.active]);
                    if !workspace.editors[app.active].page_by(true) { workspace.editors[app.active].scroll_y += 480.0; }
                }
                "save" | "save-as" => {
                    workspace.editors[app.active].enqueue(Input::Insert("PERF_SAVE".into()));
                }
                "workspace-scan" => {
                    let Some(parent) = config.input.as_ref().and_then(|path| path.parent()) else { self.finish(false); return true; };
                    workspace.search_panel.start_folder(bareline_search::folders::FolderScope::user(parent.to_owned()),
                        bareline_search::SearchQuery::literal("PERF_NEEDLE"),
                        std::sync::Arc::new(bareline_platform_windows::WindowsPathTrustProvider),
                        std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem), std::sync::Arc::new(|| {}));
                }
                "tail-append" => {
                    let bareline_app::workspace::WorkspaceEditor::Paged(editor) = &mut workspace.editors[app.active] else { self.finish(false); return true; };
                    if editor.start_follow(std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).is_err() { self.finish(false); return true; }
                    self.expected_tail = editor.snapshot().len().checked_add(13);
                    let Some(path) = config.input.clone() else { self.finish(false); return true; };
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    if std::thread::Builder::new().name("bareline-perf-append".into()).spawn(move || {
                        let result = (|| -> std::io::Result<()> {
                            let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
                            std::io::Write::write_all(&mut file, b"\nPERF_APPEND\n")?;
                            file.sync_data()
                        })().map_err(|error| error.to_string());
                        let _ = tx.send(result);
                    }).is_err() { self.finish(false); return true; }
                    self.append = Some(rx);
                }
                _ => {}
            }
            self.phase = 2;
            return false;
        }
        match config.workload.as_str() {
            "tabs-100" | "tabs-500" => {
                let target = if config.workload == "tabs-100" {100} else {500};
                if workspace.editors.len() < target {
                    if workspace.new_document().is_err() { self.finish(false); return true; }
                    let last = workspace.editors.len()-1;
                    workspace.editors[last].enqueue(Input::Insert("PERF_NEEDLE populated document\n".into()));
                    return false;
                }
                self.finish(true); true
            }
            "idle" => {
                if self.operation.is_some_and(|start| start.elapsed() >= Duration::from_secs(10)) {
                    self.finish(true); true
                } else { false }
            }
            "extensions-memory" => false, // Shell routes the owning native extension controller below.
            "syntax" => {
                if workspace.styling_receipt().is_some_and(|receipt| receipt.unavailable) { self.finish(false); return true; }
                if self.syntax_present {
                    if let Some(value) = self.syntax_present_us { self.extra.push(("syntax_ready_present_us", value)); }
                    if let Some(receipt) = workspace.styling_receipt() { self.extra.push(("syntax_viewport_bytes", receipt.range.end.0.saturating_sub(receipt.range.start.0) as u128)); }
                    self.finish(true); true
                } else { false }
            }
            "workspace-scan" => {
                if workspace.search_panel.folder_searching() { return false; }
                let Some(results) = workspace.search_panel.folder_receipt() else { self.finish(false); return true; };
                let success = results.summary.completeness == bareline_search::Completeness::Complete && results.summary.skipped_files == 0;
                self.extra.push(("workspace_searched_files", results.summary.searched_files as u128));
                self.extra.push(("workspace_matches", results.summary.count as u128));
                self.finish(success); true
            }
            "search-cancel" => {
                if self.phase == 2 {
                    if !workspace.find.searching() {
                        if workspace.find.completed_results().is_some() || workspace.find.completed_paged_results().is_some() { self.finish(false); return true; }
                        return false;
                    }
                    self.operation = Some(Instant::now());
                    self.cancelled_search = workspace.find.cancel_search_tracked();
                    self.phase = 3;
                }
                match self.cancelled_search.as_ref().and_then(|job| job.termination()) {
                    Some(bareline_search::SearchTermination::Cancelled) => { self.finish(true); true },
                    Some(bareline_search::SearchTermination::Finished) => { self.finish(false); true },
                    None => false,
                }
            }
            "scroll" => {
                if self.frame_ready {
                    self.frame_ready = false;
                    if self.frame_scroll.is_none() || self.scroll_before.is_none() || self.frame_scroll == self.scroll_before { self.finish(false); return true; }
                    if let Some(start) = self.operation { self.samples.push(start.elapsed().as_micros()); }
                    if self.samples.len() >= 120 { self.finish(true); return true; }
                    let forward = self.samples.len() % 2 == 0;
                    self.scroll_before = scroll_position(&workspace.editors[app.active]);
                    if !workspace.editors[app.active].page_by(forward) {
                        workspace.editors[app.active].scroll_y = (workspace.editors[app.active].scroll_y + if forward {480.0} else {-480.0}).max(0.0);
                    }
                    self.operation = Some(Instant::now());
                }
                false
            }
            "literal-search" | "regex-search" | "result-jump" => {
                if !self.frame_ready || workspace.find.searching() { return false; }
                if workspace.find.completed_results().is_none() && workspace.find.completed_paged_results().is_none() {
                    self.finish(false); return true;
                }
                if config.workload == "result-jump" && self.phase == 2 {
                    let count = workspace.find.completed_results().map(|results| results.count())
                        .or_else(|| workspace.find.completed_paged_results().map(|results| results.matches.len())).unwrap_or(0);
                    if count == 0 { self.finish(false); return true; }
                    workspace.find_next(app.active, false); self.operation = Some(Instant::now());
                    self.frame_ready = false; self.phase = 3; return false;
                }
                self.finish(true); true
            }
            "save" | "save-as" if self.phase == 2 => {
                if !workspace.editors[app.active].dirty() { self.finish(false); return true; }
                let path = if config.workload == "save" { config.input.clone() } else {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                    Some(config.root.join(format!("perf-save-{stamp}.txt")))
                };
                let Some(path) = path else { self.finish(false); return true; };
                self.operation = Some(Instant::now());
                workspace.save(app.active, path.clone()); self.output = Some(path); self.phase = 3; false
            }
            "save" | "save-as" => {
                let committed = self.output.as_deref().is_some_and(|path| workspace.path(app.active) == Some(path));
                let clean = committed && !workspace.editors[app.active].dirty();
                if clean && let Some(start) = self.operation { self.extra.push(("save_to_clean_ack_us", start.elapsed().as_micros())); }
                self.finish(clean); true
            }
            "tail-append" => {
                if let Some(receiver) = &self.append {
                    match receiver.try_recv() {
                        Err(std::sync::mpsc::TryRecvError::Empty) => return false,
                        Ok(Ok(())) => self.append = None,
                        _ => { self.finish(false); return true; }
                    }
                }
                let bareline_app::workspace::WorkspaceEditor::Paged(editor) = &mut workspace.editors[app.active] else { self.finish(false); return true; };
                if editor.follow_tick(std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem), true).is_err() { self.finish(false); return true; }
                if let Some(receipt) = editor.append_receipt() && self.operation.is_some() {
                    if let Some(elapsed) = receipt.applied_at.checked_duration_since(self.operation.unwrap()) {
                        if !self.extra.iter().any(|(name, _)| *name == "append_applied_us") { self.extra.push(("append_applied_us", elapsed.as_micros())); }
                    }
                }
                if self.frame_ready && self.frame_tail_len == self.expected_tail && self.expected_tail == Some(editor.snapshot().len()) && editor.viewport_ready() {
                    self.extra.push(("appended_bytes", 13)); self.finish(true); true
                } else { false }
            }
            "edit" => {
                if !self.frame_ready { return false; }
                self.finish(workspace.editors[app.active].dirty()); true
            }
            _ => { self.finish(false); true }
        }
    }
}

fn scroll_position(editor: &bareline_app::workspace::WorkspaceEditor) -> Option<(usize, u64)> {
    let origin = match editor {
        bareline_app::workspace::WorkspaceEditor::Paged(paged) => {
            if !paged.viewport_ready() { return None; }
            paged.source_offset(editor.visible_text.start, bareline_editor_surface::paged_view::SourceAffinity::After)?.0
        }
        _ => editor.visible_text.start.0,
    };
    Some((origin, editor.scroll_y.to_bits()))
}

impl super::Shell {
    pub(super) fn performance_pump(&mut self, el: &ActiveEventLoop) {
        if !self.performance.enabled() { return; }
        if !self.ensure_workspace(el) { return; }
        let finished = self.workspace.as_mut().is_some_and(|workspace| self.performance.pump(workspace, &mut self.app));
        let extension_workload = self.performance.config.as_ref().is_some_and(|config| config.requires_extensions());
        if extension_workload && self.performance.phase == 2 {
            let command = self.performance.config.as_ref().and_then(|config| config.extension_command.as_deref()).unwrap_or("");
            let identity = self.extensions.contributions().into_iter().find(|record| record.enabled
                && format!("{}/{}", record.identity.owner, record.identity.id) == command).map(|record| record.identity);
            if let Some(identity) = identity {
                self.performance.operation = Some(Instant::now());
                if self.extensions_invoke_contribution(identity).is_err() { self.performance.finish(false); }
                else {
                    self.performance.extension_generation = self.extensions.lifecycle_receipt().map(|receipt| receipt.generation);
                    self.performance.phase = 3;
                }
            }
        } else if extension_workload && self.performance.phase == 3 && let Some(receipt) = self.extensions.lifecycle_receipt() {
            if Some(receipt.generation) != self.performance.extension_generation { self.performance.finish(false); }
            else if receipt.succeeded == Some(false) { self.performance.finish(false); }
            else { match receipt.phase {
                super::extensions::ExtensionLifecyclePhase::Drained if receipt.succeeded == Some(true) => { self.performance.extra.push(("extension_generation", receipt.generation as u128)); self.performance.finish(true); },
                super::extensions::ExtensionLifecyclePhase::Rejected => self.performance.finish(false),
                _ => {},
            } }
        }
        if finished || self.performance.phase == 255 { el.exit(); }
        else {
            if let Some(window) = &self.window { window.request_redraw(); }
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10)));
        }
    }
    pub(super) fn performance_frame(&mut self) {
        if !self.performance.enabled() { return; }
        self.performance.frame_ready = self.workspace.as_ref().is_some_and(|workspace| {
            !workspace.io_busy() && !workspace.editors.is_empty() && workspace.editors.iter().all(|editor| !editor.busy())
                && workspace.editors.get(self.app.active).is_some_and(|editor| {
                    match editor {
                        bareline_app::workspace::WorkspaceEditor::Paged(paged) => paged.viewport_ready(),
                        bareline_app::workspace::WorkspaceEditor::Resident(resident) =>
                            resident.visible_text.end > resident.visible_text.start || resident.snapshot().is_empty(),
                    }
                })
        });
        if let Some(workspace) = &self.workspace && let Some(editor) = workspace.editors.get(self.app.active) {
            let (identity, visible, length) = match editor {
                bareline_app::workspace::WorkspaceEditor::Paged(paged) => {
                    use bareline_editor_surface::paged_view::SourceAffinity;
                    // A single styling receipt cannot certify an invented range across omitted fold bodies.
                    let crosses_gap = paged.source_segments().windows(2).any(|pair|
                        pair[0].source.end != pair[1].source.start
                            && editor.visible_text.start < pair[0].local.end
                            && pair[1].local.start < editor.visible_text.end);
                    let visible = if crosses_gap { None } else {
                        paged.source_offset(editor.visible_text.start, SourceAffinity::After)
                            .zip(paged.source_offset(editor.visible_text.end, SourceAffinity::Before))
                            .filter(|(start, end)| start <= end).map(|(start, end)| start..end)
                    };
                    (paged.snapshot().identity_token(), visible, paged.snapshot().len())
                }
                bareline_app::workspace::WorkspaceEditor::Resident(resident) => (resident.snapshot().identity_token(), Some(editor.visible_text.clone()), resident.snapshot().len()),
            };
            self.performance.frame_tail_len = self.performance.frame_ready.then_some(length);
            self.performance.frame_scroll = self.performance.frame_ready.then(|| scroll_position(editor)).flatten();
            self.performance.syntax_present = self.performance.frame_ready && workspace.styling_receipt().is_some_and(|receipt| {
                receipt.ready && !receipt.unavailable && receipt.identity == identity && receipt.language != bareline_syntax::Language::PlainText
                    && visible.as_ref().is_some_and(|range| receipt.range.start <= range.start && receipt.range.end >= range.end)
            });
            if self.performance.syntax_present && self.performance.syntax_present_us.is_none() {
                self.performance.syntax_present_us = self.performance.started.map(|start| start.elapsed().as_micros());
            }
        }
        if self.performance.first_frame_us.is_none() {
            self.performance.first_frame_us = self.performance.started.map(|start| start.elapsed().as_micros());
            self.performance.process_present_us = self.performance.config.as_ref().and_then(|config| config.origin_ns)
                .and_then(|origin| clock_ns().and_then(|now| now.checked_sub(origin))).map(|elapsed| elapsed / 1000);
        }
    }
}
