// SPDX-License-Identifier: MPL-2.0
use super::*;
#[derive(Default)]
pub(super) struct ShellIntegrationRuntime {
    tray: Option<bareline_platform_windows::shell_integration::TrayIcon>,
    pub keep_in_tray: bool,
    recent: std::collections::BTreeSet<PathBuf>,
    pub recent_files: RecentFiles,
    pub portable: bool,
}

/// Number of remembered files and the stable command IDs of the numbered
/// File ▸ Recent Files slots. The list persists to `data/recent.json` and is
/// kept local in portable mode instead of pushed to the Windows shell MRU.
pub(super) const RECENT_CAP: usize = 15;
pub(super) const RECENT_IDS: [&str; RECENT_CAP] = [
    "file.recent.0",
    "file.recent.1",
    "file.recent.2",
    "file.recent.3",
    "file.recent.4",
    "file.recent.5",
    "file.recent.6",
    "file.recent.7",
    "file.recent.8",
    "file.recent.9",
    "file.recent.10",
    "file.recent.11",
    "file.recent.12",
    "file.recent.13",
    "file.recent.14",
];
#[derive(Default)]
pub(super) struct RecentFiles {
    path: Option<PathBuf>,
    entries: Vec<PathBuf>,
}
impl RecentFiles {
    pub(super) fn configure(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.load();
    }
    pub(super) fn entries(&self) -> &[PathBuf] {
        &self.entries
    }
    fn load(&mut self) {
        self.entries.clear();
        let Some(path) = &self.path else {
            return;
        };
        if let Ok(text) = std::fs::read_to_string(path) {
            self.entries = decode_recent(&text)
                .into_iter()
                .map(PathBuf::from)
                .take(RECENT_CAP)
                .collect();
        }
    }
    pub(super) fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, encode_recent(&self.entries));
    }
    /// Moves `path` to the front, de-duplicating and capping at `RECENT_CAP`.
    /// Returns whether the ordered list actually changed.
    pub(super) fn record(&mut self, path: &std::path::Path) -> bool {
        if self.entries.first().is_some_and(|first| first == path) {
            return false;
        }
        self.entries.retain(|existing| existing != path);
        self.entries.insert(0, path.to_owned());
        self.entries.truncate(RECENT_CAP);
        true
    }
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }
}
/// Serialize the recent list as a JSON array of strings. A tiny hand-rolled
/// encoder keeps `recent.json` a plain JSON file without pulling a JSON crate
/// into the binary just for one flat list.
fn encode_recent(entries: &[PathBuf]) -> String {
    let mut text = String::from("[\n");
    for (index, path) in entries.iter().enumerate() {
        text.push_str("  \"");
        for ch in path.to_string_lossy().chars() {
            match ch {
                '"' => text.push_str("\\\""),
                '\\' => text.push_str("\\\\"),
                '\n' => text.push_str("\\n"),
                '\r' => text.push_str("\\r"),
                '\t' => text.push_str("\\t"),
                c if (c as u32) < 0x20 => text.push_str(&format!("\\u{:04x}", c as u32)),
                c => text.push(c),
            }
        }
        text.push('"');
        if index + 1 < entries.len() {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str("]\n");
    text
}
/// Read back every JSON string literal in the file (our array format contains
/// nothing else), unescaping the standard escapes we emit.
fn decode_recent(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut value = String::new();
        loop {
            match chars.next() {
                None | Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some('n') => value.push('\n'),
                    Some('r') => value.push('\r'),
                    Some('t') => value.push('\t'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        if let Some(scalar) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            value.push(scalar);
                        }
                    }
                    Some(other) => value.push(other),
                    None => break,
                },
                Some(other) => value.push(other),
            }
        }
        if !value.is_empty() {
            out.push(value);
        }
    }
    out
}
pub(super) fn commands() -> Vec<bareline_commands::CommandSpec> {
    let fixed = [
        ("file.reveal", "Open Containing Folder"),
        ("file.terminal", "Open Terminal Here"),
        ("file.recent.clear", "Clear Recent Files"),
        ("tray.toggle", "Keep Running in Tray"),
        ("tray.hide", "Minimize to Tray"),
        ("tray.restore", "Restore Window"),
    ];
    fixed
        .into_iter()
        .map(|(id, title)| (id, title.to_owned()))
        .chain(
            RECENT_IDS
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, format!("Recent File {}", i + 1))),
        )
        .map(|(id, title)| bareline_commands::CommandSpec {
            id: bareline_commands::CommandId(id),
            title: Box::leak(title.into_boxed_str()),
            category: "File",
            shortcut: "",
            action: Action::Contributed(bareline_commands::CommandId(id)),
        })
        .collect()
}
impl Shell {
    pub(super) fn shell_integration_command(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        if id == "file.recent.clear" {
            self.shell_integration.recent_files.clear();
            if let Some(w) = &mut self.workspace {
                w.message = Some("Recent files list cleared".into());
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        if let Some(index) = id
            .strip_prefix("file.recent.")
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            if let Some(path) = self.shell_integration.recent_files.entries().get(index).cloned() {
                if self.ensure_workspace(el) {
                    self.workspace.as_mut().unwrap().open(path);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            return true;
        }
        let result = match id {
            "file.reveal" | "file.terminal" => {
                let path = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(self.app.active))
                    .map(|p| p.to_owned());
                match path {
                    Some(path) => {
                        if id == "file.reveal" {
                            bareline_platform_windows::shell_integration::reveal(&path)
                        } else {
                            path.parent()
                                .ok_or_else(|| "File has no parent folder".to_owned())
                                .and_then(bareline_platform_windows::shell_integration::open_terminal)
                        }
                    }
                    None => Err("Save the current document first".into()),
                }
            }
            "tray.toggle" => {
                self.shell_integration.keep_in_tray = !self.shell_integration.keep_in_tray;
                if !self.shell_integration.keep_in_tray {
                    self.shell_integration.tray = None;
                }
                Ok(())
            }
            "tray.hide" => self.hide_to_tray(),
            "tray.restore" => {
                if let Some(window) = &self.window {
                    window.set_visible(true);
                    window.set_minimized(false);
                    window.focus_window();
                }
                Ok(())
            }
            _ => return false,
        };
        if let Some(w) = &mut self.workspace {
            w.message = Some(match result {
                Ok(()) => "Shell action completed".into(),
                Err(e) => e,
            });
        }
        true
    }
    pub(super) fn hide_to_tray(&mut self) -> Result<(), String> {
        let window = self.window.as_ref().ok_or("Window unavailable")?;
        if self.shell_integration.tray.is_none() {
            let handle = window.window_handle().map_err(|e| e.to_string())?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return Err("Windows handle unavailable".into());
            };
            self.shell_integration.tray = Some(bareline_platform_windows::shell_integration::TrayIcon::new(
                handle.hwnd.get(),
            )?);
        }
        window.set_visible(false);
        Ok(())
    }
    pub(super) fn shell_recent_pump(&mut self) {
        let paths: Vec<PathBuf> = match &self.workspace {
            Some(workspace) => (0..workspace.editors.len())
                .filter_map(|i| workspace.path(i).map(|p| p.to_owned()))
                .collect(),
            None => return,
        };
        let mut changed = false;
        for path in paths {
            // Our own numbered Recent Files list is persisted locally and works
            // in portable mode; the Windows shell MRU is only touched when installed.
            if self.shell_integration.recent_files.record(&path) {
                changed = true;
            }
            if !self.shell_integration.portable
                && self.shell_integration.recent.len() < 256
                && self.shell_integration.recent.insert(path.clone())
            {
                bareline_platform_windows::shell_integration::add_recent(&path, false);
            }
        }
        if changed {
            self.shell_integration.recent_files.save();
        }
    }
}

impl ShellIntegrationRuntime {
    pub(super) fn annotate_context(
        &self,
        context: &mut bareline_commands::CommandContext,
        has_path: bool,
        window_visible: bool,
    ) {
        use bareline_commands::{CommandId, CommandState};
        context.states.insert(
            CommandId("tray.toggle"),
            CommandState {
                checked: self.keep_in_tray,
                ..Default::default()
            },
        );
        if !has_path {
            for id in ["file.reveal", "file.terminal"] {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Save the current document first"));
            }
        }
        // The tray toggle to hide/restore only ever applies in one direction; the
        // other is not applicable, so it drops out of the menu rather than greying.
        let (id, reason) = if window_visible {
            ("tray.restore", "Window is already visible")
        } else {
            ("tray.hide", "Window is already hidden")
        };
        context
            .states
            .insert(CommandId(id), CommandState::not_applicable(reason));
        // Numbered Recent Files: a live path per slot, the surplus slots hidden.
        let entries = self.recent_files.entries();
        for (i, id) in RECENT_IDS.into_iter().enumerate() {
            match entries.get(i) {
                Some(path) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned());
                    // 1–9 get an Alt accelerator; the label leads with the number.
                    let label = if i < 9 {
                        format!("&{}  {}", i + 1, name)
                    } else {
                        format!("{}  {}", i + 1, name)
                    };
                    context.states.insert(
                        CommandId(id),
                        CommandState {
                            label: Some(label),
                            ..Default::default()
                        },
                    );
                }
                None => {
                    context
                        .states
                        .insert(CommandId(id), CommandState::not_applicable("No file in this slot"));
                }
            }
        }
        if entries.is_empty() {
            context.states.insert(
                CommandId("file.recent.clear"),
                CommandState::not_applicable("No recent files"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RecentFiles;
    use std::path::PathBuf;
    #[test]
    fn recent_files_round_trip_dedupe_cap_and_clear() {
        let dir = std::env::temp_dir().join(format!("bareline-recent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("recent.json");
        let _ = std::fs::remove_file(&file);
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        // Insert more than the cap; newest first, capped at 15.
        for i in 0..20 {
            assert!(recent.record(&PathBuf::from(format!("C:\\docs\\file{i}.txt"))));
        }
        assert_eq!(recent.entries().len(), 15);
        assert_eq!(recent.entries()[0], PathBuf::from("C:\\docs\\file19.txt"));
        // Re-recording the front is a no-op; re-recording an older one moves it up.
        assert!(!recent.record(&PathBuf::from("C:\\docs\\file19.txt")));
        assert!(recent.record(&PathBuf::from("C:\\docs\\file10.txt")));
        assert_eq!(recent.entries()[0], PathBuf::from("C:\\docs\\file10.txt"));
        recent.save();
        // A fresh instance reads the same order back from disk.
        let mut reloaded = RecentFiles::default();
        reloaded.configure(Some(file.clone()));
        assert_eq!(reloaded.entries(), recent.entries());
        assert_eq!(reloaded.entries().len(), 15);
        // Clear empties both memory and disk.
        reloaded.clear();
        assert!(reloaded.entries().is_empty());
        let mut after_clear = RecentFiles::default();
        after_clear.configure(Some(file));
        assert!(after_clear.entries().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
