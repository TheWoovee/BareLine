// SPDX-License-Identifier: MPL-2.0
//! Explicit generated-fixture workload driver; absent in ordinary launches.
use super::*;
use std::ffi::OsString;

#[derive(Clone)]
pub(super) struct PerformanceConfig {
    pub root: PathBuf,
    pub input: Option<PathBuf>,
    workload: String,
}

pub(super) fn parse_args(args: &[OsString]) -> Result<(Vec<OsString>, Option<PerformanceConfig>), Box<dyn std::error::Error>> {
    let (mut filtered, mut workload, mut root, mut input) = (Vec::new(), None, None, None);
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" { filtered.extend_from_slice(&args[index..]); break; }
        let slot = match args[index].to_str() {
            Some("--perf-workload") => Some(0), Some("--perf-root") => Some(1),
            Some("--perf-input") => Some(2), _ => None,
        };
        if let Some(slot) = slot {
            index += 1;
            let value = args.get(index).ok_or("Missing performance option value")?.clone();
            let target = match slot { 0 => &mut workload, 1 => &mut root, _ => &mut input };
            if target.replace(value).is_some() { return Err("Duplicate performance option".into()); }
        } else { filtered.push(args[index].clone()); }
        index += 1;
    }
    let Some(workload) = workload else {
        if root.is_some() || input.is_some() { return Err("Performance paths require --perf-workload".into()); }
        return Ok((filtered, None));
    };
    let workload = workload.into_string().map_err(|_| "Invalid workload")?;
    if !["open", "scroll", "edit", "literal-search", "regex-search", "result-jump", "save-as", "tabs-100", "tabs-500"].contains(&workload.as_str()) {
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
    if !workload.starts_with("tabs-") && input.is_none() { return Err("Workload requires --perf-input".into()); }
    if filtered.iter().any(|a| a == "--perf" || a == "--smoke" || a == "--text-prototype") {
        return Err("Performance workload cannot be combined with legacy diagnostic modes".into());
    }
    Ok((filtered, Some(PerformanceConfig { root, input, workload })))
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
            if let Some(operation) = self.operation { metrics.push(format!("\"operation_us\":{}", operation.elapsed().as_micros())); }
            if let Ok(value) = bareline_platform_windows::private_bytes() { metrics.push(format!("\"private_bytes_point\":{value}")); }
            if !self.samples.is_empty() {
                let mut ordered = self.samples.clone(); ordered.sort_unstable();
                metrics.push(format!("\"scroll_step_to_present_p50_us\":{},\"scroll_step_to_present_p95_us\":{},\"scroll_samples\":{},\"scroll_stalls_over_100ms\":{}",
                    ordered[(ordered.len()-1)/2], ordered[(ordered.len()*95).div_ceil(100)-1], ordered.len(), ordered.iter().filter(|v| **v > 100_000).count()));
            }
            println!("{{\"event\":\"measurement\",\"metrics\":{{{}}}}}", metrics.join(","));
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
            if let Some(input) = &config.input { workspace.open(input.clone()); }
            else {
                if workspace.new_document().is_err() { self.finish(false); return true; }
                workspace.editors[0].enqueue(Input::Insert("PERF_NEEDLE populated document\n".into()));
            }
            self.frame_ready = false;
            self.phase = 1;
            return false;
        }
        if workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) { return false; }
        if workspace.editors.is_empty() || workspace.editors.iter().any(|editor| editor.error.is_some()) { self.finish(false); return true; }
        app.tabs = workspace.titles();
        app.active = app.active.min(workspace.editors.len() - 1);
        if self.phase == 1 {
            if !self.frame_ready { return false; }
            self.frame_ready = false;
            self.editable_us = self.started.map(|start| start.elapsed().as_micros());
            self.operation = Some(Instant::now());
            match config.workload.as_str() {
                "open" => { self.finish(true); return true; }
                "edit" => workspace.editors[app.active].enqueue(Input::Insert("PERF_EDIT".into())),
                "literal-search" | "regex-search" | "result-jump" => {
                    // The resident Find controller must not benchmark a paged preview
                    // as if it had searched the full source.
                    if workspace.editors[app.active].paged() { self.finish(false); return true; }
                    workspace.find.show(); workspace.find.field.insert("PERF_NEEDLE");
                    if config.workload == "regex-search" { workspace.find.toggle_mode(); workspace.find.toggle_mode(); }
                }
                "scroll" => {
                    if !workspace.editors[app.active].page_by(true) { workspace.editors[app.active].scroll_y += 480.0; }
                }
                "save-as" => {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                    let path = config.root.join(format!("perf-save-{stamp}.txt"));
                    workspace.save(app.active, path.clone()); self.output = Some(path);
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
            "scroll" => {
                if self.frame_ready {
                    self.frame_ready = false;
                    if let Some(start) = self.operation { self.samples.push(start.elapsed().as_micros()); }
                    if self.samples.len() >= 120 { self.finish(true); return true; }
                    if !workspace.editors[app.active].page_by(true) { workspace.editors[app.active].scroll_y += 480.0; }
                    self.operation = Some(Instant::now());
                }
                false
            }
            "literal-search" | "regex-search" | "result-jump" => {
                if !self.frame_ready || workspace.find.searching() { return false; }
                if workspace.find.completed_results().is_none() {
                    if workspace.find.status == "Type to find" || workspace.find.status == "Searching…" { return false; }
                    self.finish(false); return true;
                }
                if config.workload == "result-jump" && self.phase == 2 {
                    if workspace.find.completed_results().is_none_or(|results| results.count() == 0) { self.finish(false); return true; }
                    workspace.find_next(app.active, false); self.operation = Some(Instant::now());
                    self.frame_ready = false; self.phase = 3; return false;
                }
                self.finish(true); true
            }
            "save-as" => {
                let committed = self.output.as_deref().is_some_and(|path| workspace.path(app.active) == Some(path));
                self.finish(committed && !workspace.editors[app.active].dirty()); true
            }
            "edit" => {
                if !self.frame_ready { return false; }
                self.finish(workspace.editors[app.active].dirty()); true
            }
            _ => { self.finish(false); true }
        }
    }
}

impl super::Shell {
    pub(super) fn performance_pump(&mut self, el: &ActiveEventLoop) {
        if !self.performance.enabled() { return; }
        if !self.ensure_workspace(el) { return; }
        let finished = self.workspace.as_mut().is_some_and(|workspace| self.performance.pump(workspace, &mut self.app));
        if finished { el.exit(); }
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
        if self.performance.first_frame_us.is_none() {
            self.performance.first_frame_us = self.performance.started.map(|start| start.elapsed().as_micros());
        }
    }
}
