// SPDX-License-Identifier: MPL-2.0
//! Startup-only, bounded configuration and lossless product CLI parsing.
use bareline_diagnostics::{StartupAction, StartupLedger};
use std::{ffi::OsString, path::PathBuf};

pub(super) struct LaunchRuntime {
    pending: Vec<PendingPath>,
    navigation: Option<(PathBuf, std::sync::mpsc::Receiver<Result<usize,String>>)>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
struct PendingPath {
    path: PathBuf,
    line: Option<u64>,
    column: u64,
    read_only: bool,
    monitor: bool,
}
impl LaunchRuntime {
    pub(super) fn new(config: &LaunchConfig) -> Self {
        let mut runtime = Self {
            pending: Vec::new(),
            navigation: None,
            cancel: Default::default(),
        };
        runtime.queue(&bareline_platform_windows::instance::OpenRequest {
            paths: config.paths.clone(),
            line: config.line,
            column: config.column,
            read_only: config.read_only,
            monitor: config.monitor,
        });
        runtime
    }
    pub(super) fn queue(
        &mut self,
        request: &bareline_platform_windows::instance::OpenRequest,
    ) -> bool {
        if self.pending.len().saturating_add(request.paths.len()) > 256 {
            return false;
        }
        self.pending
            .extend(request.paths.iter().cloned().map(|path| PendingPath {
                path,
                line: request.line,
                column: request.column.unwrap_or(1),
                read_only: request.read_only,
                monitor: request.monitor,
            }));
        true
    }
}
impl super::Shell {
    pub(super) fn launch_pump(&mut self) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let mut remaining = Vec::new();
        let mut monitors = Vec::new();
        for pending in self.launch.pending.drain(..) {
            let Some(index) = (0..workspace.editors.len())
                .find(|&index| workspace.path(index) == Some(pending.path.as_path()))
            else {
                remaining.push(pending);
                continue;
            };
            let editor = &mut workspace.editors[index];
            if !editor.paged() && !editor.snapshot().is_complete() {
                remaining.push(pending);
                continue;
            }
            if pending.read_only {
                editor.set_read_only(true);
            }
            if let Some(line) = pending.line {
                if editor.paged() {
                    if self.launch.navigation.as_ref().is_some_and(|(path,_)|path==&pending.path) {
                        let result=self.launch.navigation.as_ref().and_then(|(_,rx)|rx.try_recv().ok());
                        if let Some(result)=result {
                            self.launch.navigation=None;
                            match result {
                                Ok(offset)=>if let bareline_app::workspace::WorkspaceEditor::Paged(paged)=editor {
                                    if let Err(e)=paged.restore_selection(bareline_document::TextOffset(offset),bareline_document::TextOffset(offset)) {workspace.message=Some(e);}
                                },
                                Err(e)=>workspace.message=Some(e),
                            }
                        } else { remaining.push(pending); continue; }
                    } else {
                        if self.launch.navigation.is_none() && let bareline_app::workspace::WorkspaceEditor::Paged(paged)=editor {
                            let handle=paged.read_handle(); let column=pending.column; let cancel=self.launch.cancel.clone(); let notify=self.notify.clone();
                            let (tx,rx)=std::sync::mpsc::sync_channel(1);
                            match std::thread::Builder::new().name("bareline-launch-position".into()).spawn(move || {let result=paged_position(handle,line,column,&cancel); let _=tx.send(result); notify();}) {
                                Ok(_)=>self.launch.navigation=Some((pending.path.clone(),rx)), Err(e)=>workspace.message=Some(e.to_string()),
                            }
                        }
                        remaining.push(pending); continue;
                    }
                } else {
                    match launch_position(editor.snapshot(), line, pending.column) {
                        Ok(offset) => {
                            editor.selection.anchor = offset;
                            editor.selection.caret = offset;
                            editor.scroll_y = (line
                                .saturating_sub(1)
                                .min(editor.snapshot().line_count().saturating_sub(1) as u64)
                                as f64
                                * 20.0)
                                .max(0.0);
                        }
                        Err(error) => workspace.message = Some(error),
                    }
                }
            }
            if pending.monitor {
                monitors.push((index,pending));
            }
        }
        self.launch.pending = remaining;
        for (index,pending) in monitors {
            if let Err(error)=self.watch_start_follow(index) {
                if let Some(workspace)=&mut self.workspace{workspace.message=Some(error);}
                self.launch.pending.push(pending);
            }
        }
    }
}
impl Drop for LaunchRuntime { fn drop(&mut self) { self.cancel.store(true,std::sync::atomic::Ordering::Release); } }
fn paged_position(handle: bareline_editor_surface::paged_view::PagedReadHandle, line:u64, column:u64, cancel:&std::sync::atomic::AtomicBool)->Result<usize,String> {
    use bareline_document::{Budget,TextOffset,line_lookup::{LineTarget,LineLookupPoll},paged::{SparseLineIndex,WindowPoll}};
    let snapshot=handle.snapshot(); let budget=Budget::new(256*1024);
    let index=SparseLineIndex::new(snapshot.clone(),16,65536,&budget).map_err(|e|format!("Line index: {e:?}"))?;
    let mut lookup=index.lookup(LineTarget::Line(usize::try_from(line.saturating_sub(1)).map_err(|_|"Line number too large")?),budget.clone()).map_err(|e|format!("Line lookup: {e:?}"))?;
    let range=loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {return Err("Navigation cancelled".into());}
        match lookup.poll() {
            LineLookupPoll::Range(range)=>break range,
            LineLookupPoll::Pending(ticket)=>{if !handle.resolve_page(ticket)? {std::thread::sleep(std::time::Duration::from_millis(1));}},
            LineLookupPoll::Progress(_)=>(),
            LineLookupPoll::Failed(bareline_document::Error::OutOfBounds)=>return Ok(snapshot.len()),
            other=>return Err(format!("Line lookup: {other:?}")),
        }
    };
    let mut offset=range.start.0; let mut columns=column.saturating_sub(1);
    while offset<range.end.0 && columns>0 {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {return Err("Navigation cancelled".into());}
        let mut request=snapshot.begin_viewport(TextOffset(offset),65536,&budget).map_err(|e|format!("Column window: {e:?}"))?;
        let window=loop {match request.poll() {WindowPoll::Ready(w)=>break w,WindowPoll::Pending(ticket)=>{if !handle.resolve_page(ticket)? {std::thread::sleep(std::time::Duration::from_millis(1));}},_=>return Err("Column lookup unavailable".into()),}};
        let start=offset.saturating_sub(window.range().start.0);
        let text=&window.text()[start..];
        let mut moved=0;
        for ch in text.chars() {if columns==0 || ch=='\r' || ch=='\n' {return Ok(offset+moved);} moved+=ch.len_utf8(); columns-=1;}
        if moved==0 {break;} offset+=moved;
    }
    Ok(offset)
}

fn launch_position(
    snapshot: &bareline_document::DocumentSnapshot,
    line: u64,
    column: u64,
) -> Result<usize, String> {
    let line = usize::try_from(line.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(snapshot.line_count().saturating_sub(1));
    let range = snapshot
        .line_range(line)
        .map_err(|error| format!("Cannot navigate to the requested line: {error:?}"))?;
    if range.end.0 - range.start.0 > 64 * 1024 {
        return Err(
            "The requested line exceeds the bounded command-line navigation window.".into(),
        );
    }
    let text = snapshot
        .read(range.clone(), 64 * 1024)
        .map_err(|error| format!("Cannot navigate to the requested column: {error:?}"))?;
    let content = text.trim_end_matches(['\r', '\n']);
    let column = usize::try_from(column.saturating_sub(1)).unwrap_or(usize::MAX);
    Ok(range.start.0
        + content
            .char_indices()
            .nth(column)
            .map_or(content.len(), |(offset, _)| offset))
}

pub struct LaunchConfig {
    pub performance: Option<super::performance::PerformanceConfig>,
    pub portable: bool,
    pub settings_path: Option<PathBuf>,
    pub session_path: Option<PathBuf>,
    pub recovery_path: Option<PathBuf>,
    pub extensions_path: Option<PathBuf>,
    pub diagnostics_path: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub read_only: bool,
    pub monitor: bool,
    pub no_session: bool,
    pub no_extensions: bool,
    pub new_instance: bool,
    pub help: bool,
    pub version: bool,
    pub software: bool,
    pub hardware: bool,
    pub smoke: bool,
    pub prototype: bool,
    pub perf: bool,
}

pub fn parse(
    args: &[OsString],
    ledger: &mut StartupLedger,
) -> Result<LaunchConfig, Box<dyn std::error::Error>> {
    ledger.record(StartupAction::ParseCli);
    let (filtered, performance) = super::performance::parse_args(args)?;
    let args = &filtered;
    let mut product = Vec::new();
    let (mut software, mut hardware, mut smoke, mut prototype, mut perf) =
        (false, false, false, false, false);
    let mut after_separator = false;
    for arg in args {
        if !after_separator && arg == "--" {
            after_separator = true;
            product.push(arg.clone());
            continue;
        }
        if !after_separator {
            if arg == "--software" {
                software = true;
                continue;
            }
            if arg == "--hardware" {
                hardware = true;
                continue;
            }
            if arg == "--smoke" {
                smoke = true;
                continue;
            }
            if arg == "--text-prototype" {
                prototype = true;
                continue;
            }
            if arg == "--perf" {
                perf = true;
                continue;
            }
        }
        product.push(arg.clone());
    }
    let options = bareline_distribution::cli::parse(product)?;
    if performance.is_some() && !options.paths.is_empty() { return Err("Performance workloads reject ordinary document paths".into()); }
    if options.paths.len() > 16 || (!options.paths.is_empty() && (smoke || perf || prototype)) {
        return Err("Open up to 16 paths; diagnostic modes do not accept document paths.".into());
    }
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or("executable directory unavailable")?;
    // Empty marker is part of settings discovery, bounded/tracked like other config.
    let portable = ledger
        .read_config(
            &directory.join("bareline.portable"),
            StartupAction::ReadSettings,
            0,
        )?
        .is_some();
    let installed = std::env::var_os("APPDATA").map(|root| PathBuf::from(root).join("Bareline"));
    let root = if let Some(config) = &performance { Some(config.root.clone()) } else if portable {
        bareline_distribution::data_root(&executable, true, directory)
    } else {
        installed
    };
    let cwd = std::env::current_dir()?;
    Ok(LaunchConfig {
        portable,
        settings_path: root.as_ref().map(|p| p.join("settings.toml")),
        session_path: root.as_ref().map(|p| p.join("session.json")),
        recovery_path: root.as_ref().map(|p| p.join("recovery")),
        extensions_path: root.as_ref().map(|p| p.join("extensions")),
        diagnostics_path: if performance.is_some() { root.as_ref().map(|p|p.join("diagnostics")) } else if smoke {
            None
        } else if portable {
            root.as_ref().map(|p| p.join("diagnostics"))
        } else {
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Bareline/diagnostics"))
        },
        paths: options
            .paths
            .into_iter()
            .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
            .collect(),
        line: options.line,
        column: options.column,
        read_only: options.read_only,
        monitor: options.monitor,
        no_session: options.no_session || performance.is_some(),
        no_extensions: options.no_extensions || performance.is_some(),
        new_instance: options.new_instance || performance.is_some(),
        help: options.help,
        version: options.version,
        software,
        hardware,
        smoke,
        prototype,
        perf,
        performance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_coordinates_clamp_and_preserve_utf8_boundaries() {
        let document = bareline_document::Document::from_utf8(
            "first\r\né🙂x\n",
            bareline_document::Budget::new(4096),
            bareline_document::Budget::new(4096),
        )
        .unwrap();
        let snapshot = document.snapshot();
        assert_eq!(launch_position(&snapshot, 2, 2).unwrap(), 9);
        assert_eq!(launch_position(&snapshot, 2, 999).unwrap(), 14);
        assert_eq!(launch_position(&snapshot, 999, 999).unwrap(), 15);
    }
}
