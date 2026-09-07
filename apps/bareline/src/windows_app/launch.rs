// SPDX-License-Identifier: MPL-2.0
//! Startup-only, bounded configuration and lossless product CLI parsing.
use bareline_diagnostics::{StartupAction, StartupLedger};
use std::{ffi::OsString, path::PathBuf};

pub(super) struct LaunchRuntime {
    pending: Vec<PendingPath>,
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
                    workspace.message = Some("Line navigation is unavailable until the paged document line index is ready.".into());
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
                workspace.message = Some("Opened read-only. Live monitor ingestion is unavailable in this build; Check for External Changes and Reload remain available.".into());
            }
        }
        self.launch.pending = remaining;
    }
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
    let root = if portable {
        bareline_distribution::data_root(&executable, true, directory)
    } else {
        installed
    };
    let cwd = std::env::current_dir()?;
    Ok(LaunchConfig {
        settings_path: root.as_ref().map(|p| p.join("settings.toml")),
        session_path: root.as_ref().map(|p| p.join("session.json")),
        recovery_path: root.as_ref().map(|p| p.join("recovery")),
        extensions_path: root.as_ref().map(|p| p.join("extensions")),
        diagnostics_path: if smoke {
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
        no_session: options.no_session,
        no_extensions: options.no_extensions,
        new_instance: options.new_instance,
        help: options.help,
        version: options.version,
        software,
        hardware,
        smoke,
        prototype,
        perf,
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
