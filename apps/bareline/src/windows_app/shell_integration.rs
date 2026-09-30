// SPDX-License-Identifier: MPL-2.0
use super::*;
#[derive(Default)]
pub(super) struct ShellIntegrationRuntime {
    tray: Option<bareline_platform_windows::shell_integration::TrayIcon>,
    pub keep_in_tray: bool,
    recent: std::collections::BTreeSet<PathBuf>,
    pub recent_files: RecentFiles,
    pub portable: bool,
    rename: Option<PendingRename>,
}
/// File ▸ Rename of the active, saved document (WSP-01). The document is held
/// read-only while the file moves on disk, then stays open, bound to the new
/// name.
struct PendingRename {
    owner: u64,
    was_read_only: bool,
    source: PathBuf,
    target: PathBuf,
    worker: std::sync::mpsc::Receiver<Result<(), String>>,
}
/// The clipboard text for File ▸ Copy Full Path / File Name / Directory Path.
pub(super) fn copied_path_text(id: &str, path: &std::path::Path) -> Option<String> {
    let text = match id {
        "file.copyPath" => path.as_os_str(),
        "file.copyName" => path.file_name()?,
        "file.copyDirectory" => path.parent()?.as_os_str(),
        _ => return None,
    };
    Some(text.to_string_lossy().into_owned()).filter(|text| !text.is_empty())
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
    /// Drops `path`, a file renamed away. Returns whether it was listed.
    pub(super) fn forget(&mut self, path: &std::path::Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|existing| existing != path);
        self.entries.len() != before
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
        ("file.copyPath", "Copy Full Path"),
        ("file.copyName", "Copy File Name"),
        ("file.copyDirectory", "Copy Directory Path"),
        ("file.rename", "Rename…"),
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
        if matches!(id, "file.copyPath" | "file.copyName" | "file.copyDirectory") {
            let text = self
                .workspace
                .as_ref()
                .and_then(|w| w.path(self.app.active))
                .and_then(|path| copied_path_text(id, path));
            let message = match (text, &self.platform) {
                (Some(text), Some(platform)) => match platform.set_clipboard_text(&text) {
                    Ok(()) => format!("Copied {text}"),
                    Err(error) => format!("Could not copy: {error}"),
                },
                (None, _) => "Save the current document first".to_owned(),
                (Some(_), None) => "Clipboard unavailable".to_owned(),
            };
            if let Some(w) = &mut self.workspace {
                w.message = Some(message);
            }
            return true;
        }
        if id == "file.rename" {
            if let Err(error) = self.shell_rename_begin()
                && let Some(w) = &mut self.workspace
            {
                w.message = Some(error);
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
    /// Rename is available for a saved, clean, idle and fully loaded document.
    pub(super) fn shell_rename_annotate(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        let editor = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active));
        let reason = if self.shell_integration.rename.is_some() {
            Some("A rename is already in progress")
        } else if editor.is_some_and(|editor| editor.paged()) {
            Some("Documents open in large-file mode cannot be renamed")
        } else if editor.is_some_and(|editor| editor.dirty() || editor.busy()) {
            Some("Save the document and wait for its work to finish first")
        } else {
            None
        };
        if let Some(reason) = reason {
            context
                .states
                .insert(CommandId("file.rename"), CommandState::disabled(reason));
        }
    }
    fn shell_rename_begin(&mut self) -> Result<(), String> {
        if self.shell_integration.rename.is_some() {
            return Err("A rename is already in progress".into());
        }
        let source = self.shell_rename_source(self.app.active)?;
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let platform = self.platform.as_ref().ok_or("Window unavailable")?;
        let Some(target) = platform.save_document_file_at(&name, source.parent())? else {
            return Ok(());
        };
        self.shell_rename_start(self.app.active, target)
    }
    /// The saved path of the document at `index`, if it can be renamed now.
    fn shell_rename_source(&self, index: usize) -> Result<PathBuf, String> {
        let workspace = self.workspace.as_ref().ok_or("Open a document first")?;
        let source = workspace
            .path(index)
            .map(PathBuf::from)
            .ok_or("Save the document before renaming it")?;
        let editor = workspace.editors.get(index).ok_or("Open a document first")?;
        if editor.paged() {
            return Err("Documents open in large-file mode cannot be renamed".into());
        }
        if editor.dirty() || editor.busy() {
            return Err("Save the document and wait for its work to finish before renaming it".into());
        }
        Ok(source)
    }
    /// Move the file to `target` on a worker thread; the pump rebinds the
    /// document once the move lands.
    fn shell_rename_start(&mut self, index: usize, target: PathBuf) -> Result<(), String> {
        if self.shell_integration.rename.is_some() {
            return Err("A rename is already in progress".into());
        }
        let source = self.shell_rename_source(index)?;
        if target == source {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let notify = self.notify.clone();
        let (from, to) = (source.clone(), target.clone());
        let _worker = std::thread::Builder::new()
            .name("file-rename".into())
            .spawn(move || {
                // A handle rename: it never replaces an existing file and never
                // crosses volumes, so the moved file keeps its identity.
                let result = bareline_platform::LocalFileSystem::rename_entry(
                    &bareline_platform_windows::WindowsFileSystem,
                    &from,
                    &to,
                )
                .map_err(|error| error.to_string());
                let _ = tx.send(result);
                notify();
            })
            .map_err(|error| error.to_string())?;
        // No edit may land while the file moves.
        let editor = self
            .workspace
            .as_mut()
            .and_then(|w| w.editors.get_mut(index))
            .ok_or("Open a document first")?;
        let owner = editor.document_identity().0;
        let was_read_only = editor.read_only();
        editor.set_read_only(true);
        self.shell_integration.rename = Some(PendingRename {
            owner,
            was_read_only,
            source,
            target,
            worker: rx,
        });
        Ok(())
    }
    /// Finish a pending rename once the disk move lands. The open document is
    /// rebound to the new name rather than reopened: a same-volume rename keeps
    /// the file identity, so a reopen would be deduplicated against the old tab.
    pub(super) fn shell_rename_pump(&mut self) {
        let Some(pending) = &self.shell_integration.rename else {
            return;
        };
        let result = match pending.worker.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("Rename worker stopped".into()),
        };
        let Some(pending) = self.shell_integration.rename.take() else {
            return;
        };
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        // The owning document, or a tab that replaced it at the old path.
        let index = workspace
            .editors
            .iter()
            .position(|editor| editor.document_identity().0 == pending.owner)
            .or_else(|| (0..workspace.editors.len()).find(|&i| workspace.path(i) == Some(pending.source.as_path())));
        let renamed = format!("Renamed to {}", pending.target.display());
        let (message, writable, rebound) = match (result, index) {
            (Err(error), _) => (format!("Rename failed: {error}"), true, false),
            (Ok(()), None) => (renamed, false, false),
            (Ok(()), Some(index)) => match workspace.rebind_path(index, pending.target.clone()) {
                Ok(_) => (renamed, true, true),
                // Keep the tab read-only: a save from it would recreate the old name.
                Err(error) => (
                    format!("{renamed}, but its tab still uses the old name ({error}); close and reopen it"),
                    false,
                    false,
                ),
            },
        };
        if writable && let Some(editor) = index.and_then(|index| workspace.editors.get_mut(index)) {
            editor.set_read_only(pending.was_read_only);
        }
        workspace.message = Some(message);
        self.app.tabs = workspace.titles();
        if rebound {
            if self.shell_integration.recent_files.forget(&pending.source) {
                self.shell_integration.recent_files.save();
            }
            self.watch_forget(&pending.source);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
            for id in [
                "file.reveal",
                "file.terminal",
                "file.copyPath",
                "file.copyName",
                "file.copyDirectory",
                "file.rename",
            ] {
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
    fn copy_path_commands_copy_the_path_name_and_directory() {
        let path = PathBuf::from("C:\\docs\\notes\\todo.txt");
        let copied = |id| super::copied_path_text(id, &path);
        assert_eq!(copied("file.copyPath").as_deref(), Some("C:\\docs\\notes\\todo.txt"));
        assert_eq!(copied("file.copyName").as_deref(), Some("todo.txt"));
        assert_eq!(copied("file.copyDirectory").as_deref(), Some("C:\\docs\\notes"));
        assert_eq!(copied("file.reveal"), None);
        // A bare root has no file name to copy.
        assert_eq!(super::copied_path_text("file.copyName", &PathBuf::from("C:\\")), None);
    }
    #[test]
    fn rename_keeps_one_writable_tab_bound_to_the_new_path() {
        use bareline_app::workspace::{Input, Workspace};
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(|editor| editor.busy()) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        fn finish_rename(shell: &mut crate::windows_app::Shell) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while shell.shell_integration.rename.is_some() {
                shell.shell_rename_pump();
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        let root = std::env::temp_dir().join(format!(
            "bareline-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("before.txt");
        let target = root.join("after.txt");
        std::fs::write(&source, b"kept text").unwrap();
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.open(source.clone());
        settle(&mut workspace);
        shell.workspace = Some(workspace);
        shell.app.active = 0;

        // Begin: the document is held read-only while the worker moves the file.
        shell.shell_rename_start(0, target.clone()).unwrap();
        assert!(shell.workspace.as_ref().unwrap().editors[0].read_only());
        finish_rename(&mut shell);
        assert_eq!(shell.app.tabs, vec!["after.txt".to_owned()]);
        let workspace = shell.workspace.as_mut().unwrap();
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert_eq!(workspace.path(0), Some(target.as_path()));
        assert!(!workspace.editors[0].read_only());
        assert_eq!(
            workspace.message.as_deref(),
            Some(format!("Renamed to {}", target.display()).as_str())
        );
        assert!(!source.exists());

        // The next save writes the new name and never recreates the old one.
        workspace.editors[0].enqueue(Input::Insert("more ".into()));
        settle(workspace);
        assert!(workspace.save(0, target.clone()));
        settle(workspace);
        assert!(!workspace.editors[0].dirty(), "{:?}", workspace.message);
        let saved = std::fs::read_to_string(&target).unwrap();
        assert!(saved.contains("more ") && saved.contains("kept text"), "{saved}");
        assert!(!source.exists());

        // Renaming onto an existing file fails and leaves the tab as it was.
        std::fs::write(&source, b"occupied").unwrap();
        shell.shell_rename_start(0, source.clone()).unwrap();
        finish_rename(&mut shell);
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.path(0), Some(target.as_path()));
        assert!(!workspace.editors[0].read_only());
        assert!(
            workspace
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("Rename failed")),
            "{:?}",
            workspace.message
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"occupied");
        drop(shell);
        let _ = std::fs::remove_dir_all(&root);
    }
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
