// SPDX-License-Identifier: MPL-2.0
use super::*;
#[derive(Default)]
pub(super) struct ShellIntegrationRuntime {
    tray: Option<bareline_platform_windows::shell_integration::TrayIcon>,
    pub keep_in_tray: bool,
    recent: std::collections::BTreeSet<PathBuf>,
    pub recent_files: RecentFiles,
    /// Workspace folders, `RECENT_FOLDER_CAP` long once configured (BIZ-07).
    pub recent_folders: RecentFiles,
    /// The right-click actions last handed to the native menu, keyed by the
    /// (length, pinned) shape of both lists they were built from.
    item_actions: Option<(
        [(usize, usize); 2],
        Vec<(bareline_commands::CommandId, Vec<(u16, String)>)>,
    )>,
    pub portable: bool,
    rename: Option<PendingRename>,
    /// The portable data folder and its recovery folder, until a worker decides
    /// after the first frame whether the folder accepts writes (APP-13).
    pub portable_data: Option<(PathBuf, Option<PathBuf>)>,
    portable_probe: Option<bareline_app::task::Task<PortableProbe>>,
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
/// How status messages name a file or folder: its name, or the whole path for a root.
fn recent_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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
/// Workspace folders opened from File ▸ Open Folder, listed under File ▸
/// Recent Files ▸ Recent Folders and kept in `data/recent-folders.json` (BIZ-07).
pub(super) const RECENT_FOLDER_CAP: usize = 8;
pub(super) const RECENT_FOLDER_IDS: [&str; RECENT_FOLDER_CAP] = [
    "file.recent.folder.0",
    "file.recent.folder.1",
    "file.recent.folder.2",
    "file.recent.folder.3",
    "file.recent.folder.4",
    "file.recent.folder.5",
    "file.recent.folder.6",
    "file.recent.folder.7",
];
/// Right-click actions of a numbered Recent slot, posted back as the high word
/// of the slot's `WM_COMMAND` (0 and 1 are a click and an accelerator).
pub(super) const RECENT_ACTION_PIN: u16 = 0x10;
pub(super) const RECENT_ACTION_REMOVE: u16 = 0x11;
/// A numbered Recent list (files or folders). Nothing here touches the disk on
/// the UI thread: the stored list is read by a worker after the first frame
/// (ADR-33, APP-11) and every change is written atomically by a worker (APP-12).
///
/// Pinned entries lead the list in the order they were pinned; opening a file
/// never moves them, and the cap only ever drops unpinned entries (BIZ-07).
pub(super) struct RecentFiles {
    path: Option<PathBuf>,
    entries: Vec<PathBuf>,
    /// How many entries at the front are pinned.
    pinned: usize,
    cap: usize,
    load: Option<bareline_app::task::Task<(usize, Vec<PathBuf>)>>,
    /// The stored list was read, or has nothing left to add.
    loaded: bool,
    /// A change made before the stored list arrived, written once it merges.
    deferred: bool,
    /// Paths renamed away before the stored list arrived, kept out of the merge.
    forgotten: Vec<PathBuf>,
    /// Clear Unpinned ran before the stored list arrived: merge only its pins.
    drop_stored_unpinned: bool,
    writer: std::sync::Arc<std::sync::Mutex<RecentWriter>>,
}
impl Default for RecentFiles {
    fn default() -> Self {
        Self::with_cap(RECENT_CAP)
    }
}
/// The newest unwritten list and whether a worker is writing; a burst of
/// changes costs one write of the latest list.
#[derive(Default)]
struct RecentWriter {
    pending: Option<(usize, Vec<PathBuf>)>,
    running: bool,
}
impl RecentFiles {
    pub(super) fn with_cap(cap: usize) -> Self {
        Self {
            path: None,
            entries: Vec::new(),
            pinned: 0,
            cap,
            load: None,
            loaded: false,
            deferred: false,
            forgotten: Vec::new(),
            drop_stored_unpinned: false,
            writer: Default::default(),
        }
    }
    /// Remember where the list lives. Nothing is read yet (ADR-33).
    pub(super) fn configure(&mut self, path: Option<PathBuf>) {
        self.loaded = path.is_none();
        self.load = None;
        self.path = path;
    }
    pub(super) fn entries(&self) -> &[PathBuf] {
        &self.entries
    }
    /// Number of pinned entries, which lead [`Self::entries`].
    pub(super) fn pinned_len(&self) -> usize {
        self.pinned
    }
    pub(super) fn is_pinned(&self, path: &std::path::Path) -> bool {
        self.entries[..self.pinned].iter().any(|entry| entry == path)
    }
    /// Pins may fill two thirds of the list, so recent entries always show.
    fn pin_limit(&self) -> usize {
        (self.cap - self.cap / 3).max(1)
    }
    /// Read the stored list on a worker; `notify` wakes the loop when it lands.
    pub(super) fn start_load(&mut self, notify: impl Fn() + Send + 'static) {
        if self.loaded || self.load.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        let cap = self.cap;
        match bareline_app::task::spawn(notify, move |_| read_recent(&path, cap)) {
            Ok(task) => self.load = Some(task),
            // Without a worker the stored list stays unread; recording still works.
            Err(_) => self.loaded = true,
        }
    }
    /// Merge the stored list once it arrives, behind anything recorded since
    /// startup. Stored pins stay pinned, even for a file opened meanwhile.
    /// Returns whether the list changed.
    pub(super) fn poll(&mut self) -> bool {
        let Some(load) = &self.load else {
            return false;
        };
        let (stored_pinned, stored) = match load.poll() {
            bareline_app::task::TaskPoll::Pending => return false,
            bareline_app::task::TaskPoll::Complete(stored) => stored,
            _ => (0, Vec::new()),
        };
        self.load = None;
        self.loaded = true;
        let before = (self.pinned, self.entries.clone());
        let forgotten = std::mem::take(&mut self.forgotten);
        let drop_unpinned = std::mem::take(&mut self.drop_stored_unpinned);
        let mut pins = self.entries[..self.pinned].to_vec();
        for path in &stored[..stored_pinned.min(stored.len())] {
            if !forgotten.contains(path) && !pins.contains(path) && pins.len() < self.pin_limit() {
                pins.push(path.clone());
            }
        }
        let mut recent: Vec<PathBuf> = self.entries[self.pinned..]
            .iter()
            .filter(|path| !pins.contains(path))
            .cloned()
            .collect();
        if !drop_unpinned {
            for path in stored {
                if !forgotten.contains(&path) && !pins.contains(&path) && !recent.contains(&path) {
                    recent.push(path);
                }
            }
        }
        self.pinned = pins.len();
        self.entries = pins;
        self.entries.extend(recent);
        self.entries.truncate(self.cap);
        (self.pinned, &self.entries) != (before.0, &before.1) || std::mem::take(&mut self.deferred)
    }
    /// Record opened, saved or closed paths, oldest first, so the last one ends
    /// up on top. Returns whether the ordered list changed.
    pub(super) fn apply(&mut self, events: &[PathBuf]) -> bool {
        let mut changed = false;
        for path in events {
            changed |= self.record(path);
        }
        changed
    }
    /// Moves `path` to the top of the unpinned entries, de-duplicating and
    /// capping; a pinned path keeps its place. Returns whether the ordered list
    /// actually changed.
    pub(super) fn record(&mut self, path: &std::path::Path) -> bool {
        match self.entries.iter().position(|existing| existing == path) {
            Some(index) if index <= self.pinned => return false,
            Some(index) => {
                self.entries.remove(index);
            }
            None => {}
        }
        self.entries.insert(self.pinned, path.to_owned());
        self.entries.truncate(self.cap);
        true
    }
    /// Pin `path` below the pins already there, adding it when it is not
    /// listed, or unpin it to the top of the recent entries. Returns whether
    /// the list changed, or why a pin was refused.
    pub(super) fn set_pinned(&mut self, path: &std::path::Path, pinned: bool) -> Result<bool, String> {
        let index = self.entries.iter().position(|existing| existing == path);
        if pinned {
            if index.is_some_and(|index| index < self.pinned) {
                return Ok(false);
            }
            if self.pinned >= self.pin_limit() {
                return Err(format!(
                    "At most {} items can be pinned. Unpin one first.",
                    self.pin_limit()
                ));
            }
            if let Some(index) = index {
                self.entries.remove(index);
            }
            self.forgotten.retain(|existing| existing != path);
            self.entries.insert(self.pinned, path.to_owned());
            self.pinned += 1;
        } else {
            let Some(index) = index.filter(|index| *index < self.pinned) else {
                return Ok(false);
            };
            let entry = self.entries.remove(index);
            self.pinned -= 1;
            self.entries.insert(self.pinned, entry);
        }
        self.entries.truncate(self.cap);
        Ok(true)
    }
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.pinned = 0;
        // A stored list still being read must not come back after a clear.
        self.load = None;
        self.loaded = true;
        self.forgotten.clear();
        self.drop_stored_unpinned = false;
        self.save();
    }
    /// Drop every unpinned entry. Returns whether the list needs writing.
    pub(super) fn clear_unpinned(&mut self) -> bool {
        let changed = self.entries.len() > self.pinned || !self.loaded;
        self.entries.truncate(self.pinned);
        if !self.loaded {
            self.drop_stored_unpinned = true;
        }
        changed
    }
    /// Write the current list atomically on a worker. A list whose stored copy
    /// was never read is not written, so it cannot overwrite what it never saw.
    pub(super) fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if !self.loaded {
            self.deferred = true;
            return;
        }
        {
            let mut writer = self.writer.lock().unwrap_or_else(|error| error.into_inner());
            writer.pending = Some((self.pinned, self.entries.clone()));
            if writer.running {
                return;
            }
            writer.running = true;
        }
        let writer = self.writer.clone();
        let job = move || {
            loop {
                let (pinned, entries) = {
                    let mut state = writer.lock().unwrap_or_else(|error| error.into_inner());
                    match state.pending.take() {
                        Some(pending) => pending,
                        None => {
                            state.running = false;
                            return;
                        }
                    }
                };
                if let Err(error) = write_recent(&path, pinned, &entries) {
                    eprintln!("event=recent_files_write_failed kind={:?}", error.kind());
                }
            }
        };
        if bareline_app::task::execute(job).is_err() {
            // The next change tries again with the newest list.
            self.writer.lock().unwrap_or_else(|error| error.into_inner()).running = false;
        }
    }
    /// Drops `path`: a file renamed away, or one the person removed from the
    /// list. Returns whether the list needs writing: it was listed, or the
    /// stored list has not merged yet and must be written without it once it does.
    pub(super) fn forget(&mut self, path: &std::path::Path) -> bool {
        let index = self.entries.iter().position(|existing| existing == path);
        if let Some(index) = index {
            self.entries.remove(index);
            if index < self.pinned {
                self.pinned -= 1;
            }
        }
        if !self.loaded {
            // The stored list may still name the old path; keep it out of the merge.
            self.forgotten.push(path.to_owned());
            return true;
        }
        index.is_some()
    }
    /// Whether a list is waiting to be written or being written.
    #[cfg(test)]
    fn writing(&self) -> bool {
        let writer = self.writer.lock().unwrap_or_else(|error| error.into_inner());
        writer.running || writer.pending.is_some()
    }
}
fn read_recent(path: &std::path::Path, cap: usize) -> (usize, Vec<PathBuf>) {
    std::fs::read_to_string(path)
        .map(|text| {
            let (pinned, entries) = decode_recent(&text);
            let entries: Vec<PathBuf> = entries.into_iter().map(PathBuf::from).take(cap).collect();
            (pinned.min(entries.len()), entries)
        })
        .unwrap_or_default()
}
/// Stage beside the target and rename over it, so a crash or a full disk never
/// leaves a truncated list. The stage is named after the list, so the files
/// and folders lists can be written at the same time.
fn write_recent(path: &std::path::Path, pinned: usize, entries: &[PathBuf]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stage = path.with_file_name(format!(".{name}-{}.tmp", std::process::id()));
    let result = std::fs::File::create(&stage)
        .and_then(|mut file| {
            file.write_all(encode_recent(pinned, entries).as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&stage, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&stage);
    }
    result
}
/// Serialize the recent list as a JSON array of strings. A tiny hand-rolled
/// encoder keeps `recent.json` a plain JSON file without pulling a JSON crate
/// into the binary just for one flat list. When entries are pinned, a leading
/// number counts them (`[2, "a", "b", "c"]`); older builds skip it and read
/// every path, unpinned.
fn encode_recent(pinned: usize, entries: &[PathBuf]) -> String {
    let mut text = String::from("[\n");
    if pinned > 0 {
        text.push_str(&format!("  {pinned}"));
        text.push_str(if entries.is_empty() { "\n" } else { ",\n" });
    }
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
fn decode_recent(text: &str) -> (usize, Vec<String>) {
    // The optional pin count directly after the opening bracket.
    let pinned = text
        .trim_start()
        .strip_prefix('[')
        .map(|rest| {
            let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<usize>().unwrap_or(0)
        })
        .unwrap_or(0);
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
    (pinned.min(out.len()), out)
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
        ("file.recent.clearUnpinned", "Clear Unpinned Recent Files"),
        ("file.recent.pin", "Pin Current File to Recent Files"),
        ("file.recent.remove", "Remove Current File from Recent Files"),
        ("file.recent.folder.clear", "Clear Recent Folders"),
        ("file.session.load", "Load Session…"),
        ("file.session.save", "Save Session As…"),
        ("file.openNewInstance", "Open in New Instance"),
        ("file.moveNewInstance", "Move to New Instance"),
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
        .chain(
            RECENT_FOLDER_IDS
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, format!("Recent Folder {}", i + 1))),
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
        if id.starts_with("file.session.") {
            self.session_named_command(id);
            return true;
        }
        if matches!(id, "file.openNewInstance" | "file.moveNewInstance") {
            let result = self.shell_new_instance(el, id == "file.moveNewInstance");
            if let Some(w) = &mut self.workspace {
                w.message = Some(match result {
                    Ok(message) | Err(message) => message,
                });
            }
            return true;
        }
        if let Some(message) = self.shell_recent_command(id) {
            if let Some(w) = &mut self.workspace {
                w.message = Some(message);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        if let Some(index) = id
            .strip_prefix("file.recent.folder.")
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            if let Some(path) = self.shell_integration.recent_folders.entries().get(index).cloned()
                && !self.panels_open_root(path)
                && let Some(w) = &mut self.workspace
            {
                w.message = Some("Wait for the folder being opened to finish".into());
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
    /// Rename is available for a saved, clean, idle and fully loaded document,
    /// and renames the tab title of an Untitled one (WSP-01).
    pub(super) fn shell_rename_annotate(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        let editor = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active));
        if let Some(workspace) = &self.workspace
            && editor.is_some()
            && workspace.path(self.app.active).is_none()
        {
            let state = match workspace.untitled_rename_blocked(self.app.active) {
                None => CommandState {
                    label: Some("Rename Tab…".to_owned()),
                    ..Default::default()
                },
                Some(reason) => CommandState::disabled(reason),
            };
            context.states.insert(CommandId("file.rename"), state);
            return;
        }
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
        // An Untitled tab has no file to move: rename its title instead.
        if let Some(workspace) = &self.workspace
            && workspace.editors.get(self.app.active).is_some()
            && workspace.path(self.app.active).is_none()
        {
            if let Some(reason) = workspace.untitled_rename_blocked(self.app.active) {
                return Err(reason.into());
            }
            self.goto_rename_untitled(self.app.active);
            return Ok(());
        }
        let source = self.shell_rename_source(self.app.active)?;
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let platform = self.platform.as_ref().ok_or("Window unavailable")?;
        // The exact name is kept, and the rename worker refuses an existing target.
        let options = bareline_platform::SaveDialogOptions::new(bareline_platform::SaveFileKind::Named)
            .named(name)
            .in_directory(source.parent().map(PathBuf::from))
            .app_confirms_overwrite();
        let Some(target) = platform.save_file_with(&options)? else {
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
    /// Start reading the stored Recent Files and Recent Folders lists; called
    /// after the first frame.
    pub(super) fn shell_recent_start(&mut self) {
        let wake = self.wake.clone();
        self.shell_integration
            .recent_files
            .start_load(move || wake(Wake::One(Source::ShellRecent)));
        let wake = self.wake.clone();
        self.shell_integration
            .recent_folders
            .start_load(move || wake(Wake::One(Source::ShellRecent)));
    }
    /// A workspace folder opened: it goes to the top of Recent Folders.
    pub(super) fn shell_recent_folder_opened(&mut self, path: &std::path::Path) {
        if self.shell_integration.recent_folders.record(path) {
            self.shell_integration.recent_folders.save();
        }
    }
    /// Clear, pin and remove in the Recent lists. Returns the status message,
    /// or `None` when `id` is not one of these commands (BIZ-07).
    fn shell_recent_command(&mut self, id: &str) -> Option<String> {
        let active = self
            .workspace
            .as_ref()
            .and_then(|w| w.path(self.app.active))
            .map(PathBuf::from);
        let lists = &mut self.shell_integration;
        let message = match id {
            "file.recent.clear" => {
                lists.recent_files.clear();
                "Recent files list cleared".to_owned()
            }
            "file.recent.folder.clear" => {
                lists.recent_folders.clear();
                "Recent folders list cleared".to_owned()
            }
            "file.recent.clearUnpinned" => {
                if lists.recent_files.clear_unpinned() {
                    lists.recent_files.save();
                }
                "Unpinned recent files cleared".to_owned()
            }
            "file.recent.pin" | "file.recent.remove" => {
                let Some(path) = active else {
                    return Some("Save the current document first".to_owned());
                };
                let result = if id == "file.recent.remove" {
                    Ok((lists.recent_files.forget(&path), "Removed"))
                } else {
                    let pin = !lists.recent_files.is_pinned(&path);
                    lists
                        .recent_files
                        .set_pinned(&path, pin)
                        .map(|changed| (changed, if pin { "Pinned" } else { "Unpinned" }))
                };
                match result {
                    Ok((changed, verb)) => {
                        if changed {
                            lists.recent_files.save();
                        }
                        format!("{verb} {}", recent_name(&path))
                    }
                    Err(error) => error,
                }
            }
            _ => return None,
        };
        Some(message)
    }
    /// A Pin/Unpin or Remove chosen from the right-click menu of a numbered
    /// Recent Files or Recent Folders slot (BIZ-07).
    pub(super) fn shell_recent_item_action(&mut self, id: &str, action: u16) {
        let (list, index) = if let Some(index) = id
            .strip_prefix("file.recent.folder.")
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            (&mut self.shell_integration.recent_folders, index)
        } else if let Some(index) = id
            .strip_prefix("file.recent.")
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            (&mut self.shell_integration.recent_files, index)
        } else {
            return;
        };
        let Some(path) = list.entries().get(index).cloned() else {
            return;
        };
        let result = match action {
            RECENT_ACTION_PIN => {
                let pin = !list.is_pinned(&path);
                list.set_pinned(&path, pin)
                    .map(|changed| (changed, if pin { "Pinned" } else { "Unpinned" }))
            }
            RECENT_ACTION_REMOVE => Ok((list.forget(&path), "Removed")),
            _ => return,
        };
        let message = match result {
            Ok((changed, verb)) => {
                if changed {
                    list.save();
                }
                format!("{verb} {}", recent_name(&path))
            }
            Err(error) => error,
        };
        if let Some(w) = &mut self.workspace {
            w.message = Some(message);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// States of the current-file Recent commands and of Open/Move to New
    /// Instance, which need a saved, clean document (BIZ-07).
    pub(super) fn shell_file_annotate(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        let workspace = self.workspace.as_ref();
        let active = workspace.and_then(|w| w.path(self.app.active));
        let files = &self.shell_integration.recent_files;
        context.states.insert(
            CommandId("file.recent.pin"),
            match active {
                Some(path) => CommandState {
                    checked: files.is_pinned(path),
                    label: Some("Pin Current File".to_owned()),
                    ..Default::default()
                },
                None => CommandState::not_applicable("Save the current document first"),
            },
        );
        if !active.is_some_and(|path| files.entries().iter().any(|entry| entry == path)) {
            context.states.insert(
                CommandId("file.recent.remove"),
                CommandState::disabled("The current file is not in Recent Files"),
            );
        }
        let dirty = workspace
            .and_then(|w| w.editors.get(self.app.active))
            .is_some_and(|editor| editor.dirty());
        let reason = if active.is_none() {
            Some("Save the document first; only saved files open in another window")
        } else if dirty {
            Some("Save the changes first; unsaved text cannot move to another window")
        } else {
            None
        };
        if let Some(reason) = reason {
            for id in ["file.openNewInstance", "file.moveNewInstance"] {
                context.states.insert(CommandId(id), CommandState::disabled(reason));
            }
        }
    }
    /// Open the active document in a separate Bareline window, and for Move,
    /// close it here. The instance handoff carries paths only, so unsaved text
    /// cannot cross: both need a saved document without unsaved changes.
    fn shell_new_instance(&mut self, el: &ActiveEventLoop, close: bool) -> Result<String, String> {
        let index = self.app.active;
        let workspace = self.workspace.as_ref().ok_or("Open a document first")?;
        let path = workspace
            .path(index)
            .map(PathBuf::from)
            .ok_or("Save the document first; only saved files open in another window")?;
        let editor = workspace.editors.get(index).ok_or("Open a document first")?;
        if editor.dirty() {
            return Err("Save the changes first; unsaved text cannot move to another window".into());
        }
        // The new window opens at the caret's line.
        let line = editor.resident().and_then(|surface| {
            surface
                .snapshot()
                .line_at(bareline_document::TextOffset(surface.selection.caret))
                .ok()
        });
        let mut command = std::process::Command::new(
            std::env::current_exe().map_err(|error| format!("Could not start a new window: {error}"))?,
        );
        command.arg("--new-instance");
        if let Some(line) = line {
            command.arg("--line").arg((line + 1).to_string());
        }
        command.arg("--").arg(&path);
        command
            .spawn()
            .map_err(|error| format!("Could not start a new window: {error}"))?;
        if close {
            self.dispatch(el, Action::Close);
            return Ok(format!("Moved {} to a new window", recent_name(&path)));
        }
        Ok(format!("Opened {} in a new window", recent_name(&path)))
    }
    pub(super) fn shell_recent_pump(&mut self) {
        if self.shell_integration.recent_folders.poll() {
            self.shell_integration.recent_folders.save();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        let mut changed = self.shell_integration.recent_files.poll();
        // Only opens, saves and closes reorder the list; open tabs are never
        // re-recorded on a wake (APP-12).
        let paths = self
            .workspace
            .as_mut()
            .map(|workspace| workspace.take_recent_events())
            .unwrap_or_default();
        changed |= self.shell_integration.recent_files.apply(&paths);
        // The user can keep opened paths out of Windows Recent items (PRIVACY.md).
        let shell_recent = self.settings.effective().add_to_windows_recent;
        for path in paths {
            // Our own numbered Recent Files list is persisted locally and works
            // in portable mode; the Windows shell MRU is only touched when installed.
            if shell_recent
                && !self.shell_integration.portable
                && self.shell_integration.recent.len() < 256
                && self.shell_integration.recent.insert(path.clone())
            {
                bareline_platform_windows::shell_integration::add_recent(
                    &path,
                    self.shell_integration.portable,
                    shell_recent,
                );
            }
        }
        if changed {
            self.shell_integration.recent_files.save();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
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
        recent_slot_states(context, &RECENT_IDS, &self.recent_files, false);
        recent_slot_states(context, &RECENT_FOLDER_IDS, &self.recent_folders, true);
        let entries = self.recent_files.entries();
        if entries.is_empty() {
            context.states.insert(
                CommandId("file.recent.clear"),
                CommandState::not_applicable("No recent files"),
            );
        }
        // With nothing pinned, Clear Recent Files already does this.
        if self.recent_files.pinned_len() == 0 || entries.len() == self.recent_files.pinned_len() {
            context.states.insert(
                CommandId("file.recent.clearUnpinned"),
                CommandState::not_applicable("No pinned and unpinned files to tell apart"),
            );
        }
        if self.recent_folders.entries().is_empty() {
            context.states.insert(
                CommandId("file.recent.folder.clear"),
                CommandState::not_applicable("No recent folders"),
            );
        }
    }
    /// The Pin/Unpin and Remove right-click actions of every listed Recent slot,
    /// rebuilt only when either list changed shape (BIZ-07).
    pub(super) fn recent_item_actions(&mut self) -> &[(bareline_commands::CommandId, Vec<(u16, String)>)] {
        let key = [
            (self.recent_files.entries().len(), self.recent_files.pinned_len()),
            (self.recent_folders.entries().len(), self.recent_folders.pinned_len()),
        ];
        if self.item_actions.as_ref().is_none_or(|(built, _)| *built != key) {
            let mut actions = Vec::new();
            for (ids, (len, pinned)) in [&RECENT_IDS[..], &RECENT_FOLDER_IDS[..]].into_iter().zip(key) {
                for (index, id) in ids.iter().enumerate().take(len) {
                    let pin = if index < pinned { "Unpin" } else { "Pin to Top" };
                    actions.push((
                        bareline_commands::CommandId(*id),
                        vec![
                            (RECENT_ACTION_PIN, pin.to_owned()),
                            (RECENT_ACTION_REMOVE, "Remove from List".to_owned()),
                        ],
                    ));
                }
            }
            self.item_actions = Some((key, actions));
        }
        self.item_actions
            .as_ref()
            .map(|(_, actions)| actions.as_slice())
            .unwrap_or(&[])
    }
}
/// Label the numbered slots of `list`, hiding the empty ones. Files show their
/// name and folders their full path; pinned entries say so.
fn recent_slot_states(
    context: &mut bareline_commands::CommandContext,
    ids: &[&'static str],
    list: &RecentFiles,
    folders: bool,
) {
    use bareline_commands::{CommandId, CommandState};
    for (i, id) in ids.iter().enumerate() {
        match list.entries().get(i) {
            Some(path) => {
                let name = path
                    .file_name()
                    .filter(|_| !folders)
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let pinned = if i < list.pinned_len() { "  (pinned)" } else { "" };
                // 1–9 get an Alt accelerator; the label leads with the number.
                let label = if i < 9 {
                    format!("&{}  {name}{pinned}", i + 1)
                } else {
                    format!("{}  {name}{pinned}", i + 1)
                };
                context.states.insert(
                    CommandId(*id),
                    CommandState {
                        label: Some(label),
                        ..Default::default()
                    },
                );
            }
            None => {
                context.states.insert(
                    CommandId(*id),
                    CommandState::not_applicable(if folders {
                        "No folder in this slot"
                    } else {
                        "No file in this slot"
                    }),
                );
            }
        }
    }
}

/// What the portable data folder allows, decided on a worker after the first
/// frame (APP-13).
pub(super) struct PortableProbe {
    /// `Err` says why the folder refused a write.
    writable: Result<(), String>,
    /// The local folder that keeps recovery journals when the data folder is
    /// read-only, once it exists.
    fallback: Option<PathBuf>,
}
/// Create and remove a file: the one test that the media, share permissions and
/// the read-only attribute all allow writes.
fn probe_writable(root: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let probe = root.join(format!(".bareline-write-probe-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}
fn run_portable_probe(root: &std::path::Path, fallback: Option<PathBuf>) -> PortableProbe {
    let writable = probe_writable(root);
    let fallback = writable
        .is_err()
        .then_some(fallback)
        .flatten()
        .filter(|fallback| std::fs::create_dir_all(fallback).is_ok());
    PortableProbe { writable, fallback }
}
/// A local profile folder for recovery journals of a read-only portable copy,
/// one per portable data folder so separate copies never share journals.
fn portable_fallback_root(portable: &std::path::Path) -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|root| {
            PathBuf::from(root)
                .join("Bareline")
                .join("portable-recovery")
                .join(portable_recovery_key(portable))
        })
        .filter(|root| root.is_absolute())
}
/// FNV-1a over the case-folded folder path: stable across runs and Rust
/// releases, unlike the standard library hasher.
fn portable_recovery_key(portable: &std::path::Path) -> String {
    let hash = portable
        .to_string_lossy()
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{hash:016x}")
}
impl Shell {
    /// Check that the portable data folder accepts writes, on a worker after the
    /// first frame (APP-13). Recovery journals wait for the answer.
    pub(super) fn portable_probe_start(&mut self) {
        let Some((root, _)) = self.shell_integration.portable_data.clone() else {
            return;
        };
        if self.shell_integration.portable_probe.is_some() {
            return;
        }
        let fallback = portable_fallback_root(&root);
        let wake = self.wake.clone();
        match bareline_app::task::spawn(
            move || wake(Wake::One(Source::Recovery)),
            move |_| run_portable_probe(&root, fallback),
        ) {
            Ok(task) => self.shell_integration.portable_probe = Some(task),
            // Without a worker, keep the portable folder as before the check.
            Err(_) => self.portable_decided(PortableProbe {
                writable: Ok(()),
                fallback: None,
            }),
        }
    }
    pub(super) fn portable_probe_pump(&mut self) {
        let Some(task) = &self.shell_integration.portable_probe else {
            return;
        };
        let probe = match task.poll() {
            bareline_app::task::TaskPoll::Pending => return,
            bareline_app::task::TaskPoll::Complete(probe) => probe,
            _ => PortableProbe {
                writable: Err("The folder check did not finish.".into()),
                fallback: None,
            },
        };
        self.shell_integration.portable_probe = None;
        self.portable_decided(probe);
    }
    fn portable_decided(&mut self, probe: PortableProbe) {
        let Some((root, recovery)) = self.shell_integration.portable_data.take() else {
            return;
        };
        let recovery_root = match probe.writable {
            Ok(()) => recovery,
            Err(reason) => {
                // Every exit would otherwise fail to save the session and need a
                // second close, and recovery would fail without a word.
                self.session.disable_persistence();
                let mut journals = match &probe.fallback {
                    Some(fallback) => format!("Recovery journals are kept in {}.", fallback.display()),
                    None => "Recovery journals cannot be kept on this computer.".to_owned(),
                };
                // Only one recovery folder is searched; earlier journals on the
                // portable media wait there until it is writable again.
                if let Some(recovery) = &recovery {
                    journals.push_str(&format!(
                        " Journals already in {} are offered again once the folder is writable.",
                        recovery.display()
                    ));
                }
                self.startup_notice(
                    "portable:read-only",
                    bareline_ui::theme::ToastLevel::Warning,
                    "The portable data folder is read-only.".into(),
                    format!(
                        "{}: {reason}\nThe session, settings changes and recent files are not saved. {journals}",
                        root.display()
                    ),
                );
                probe.fallback
            }
        };
        if let Some(workspace) = &mut self.workspace {
            workspace.recovery_root = recovery_root.clone();
        }
        self.recovery_root = recovery_root.clone();
        self.recovery.configure(recovery_root, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn temp_dir(name: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let dir = std::env::temp_dir().join(format!(
            "bareline-recent-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    /// Drive the worker read to completion; returns whether any poll merged.
    fn settle_load(recent: &mut RecentFiles) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut merged = false;
        while !recent.loaded {
            merged |= recent.poll();
            assert!(Instant::now() < deadline, "the stored list was never read");
            std::thread::yield_now();
        }
        merged
    }
    fn settle_write(recent: &RecentFiles) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while recent.writing() {
            assert!(Instant::now() < deadline, "the list was never written");
            std::thread::yield_now();
        }
    }
    fn stored(file: &std::path::Path) -> Vec<PathBuf> {
        read_recent(file, RECENT_CAP).1
    }
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
        let dir = temp_dir("round-trip");
        let file = dir.join("recent.json");
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        recent.start_load(|| {});
        settle_load(&mut recent);
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
        settle_write(&recent);
        // A fresh instance reads the same order back from disk.
        let mut reloaded = RecentFiles::default();
        reloaded.configure(Some(file.clone()));
        reloaded.start_load(|| {});
        settle_load(&mut reloaded);
        assert_eq!(reloaded.entries(), recent.entries());
        // Clear empties both memory and disk.
        reloaded.clear();
        settle_write(&reloaded);
        assert!(reloaded.entries().is_empty());
        assert!(stored(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// APP-11: configuring reads nothing; the worker read lands later, behind
    /// the paths recorded first, and the merged list is then written once.
    #[test]
    fn stored_list_loads_on_a_worker_behind_paths_recorded_first() {
        let dir = temp_dir("lazy");
        let file = dir.join("recent.json");
        let (a, b, c) = (
            PathBuf::from("C:\\a.txt"),
            PathBuf::from("C:\\b.txt"),
            PathBuf::from("C:\\c.txt"),
        );
        write_recent(&file, 0, &[a.clone(), b.clone()]).unwrap();
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        assert!(recent.entries().is_empty(), "nothing is read before the first frame");
        assert!(recent.apply(std::slice::from_ref(&c)));
        recent.save();
        assert!(!recent.writing(), "an unread list is never written over the stored one");
        assert_eq!(stored(&file), [a.clone(), b.clone()]);
        recent.start_load(|| {});
        assert!(settle_load(&mut recent), "the merge asks for one write");
        assert_eq!(recent.entries(), [c.clone(), a.clone(), b.clone()]);
        recent.save();
        settle_write(&recent);
        assert_eq!(stored(&file), [c, a, b]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// WSP-01 with APP-11: a path renamed away is dropped from the list, and a
    /// rename before the stored list arrives keeps the old name out of the merge.
    #[test]
    fn renamed_path_is_forgotten_before_and_after_the_stored_list_loads() {
        let dir = temp_dir("forget");
        let file = dir.join("recent.json");
        let (a, b, c) = (
            PathBuf::from("C:\\a.txt"),
            PathBuf::from("C:\\b.txt"),
            PathBuf::from("C:\\c.txt"),
        );
        write_recent(&file, 0, &[a.clone(), b.clone()]).unwrap();
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        assert!(recent.forget(&a), "an unread list must be written without it");
        recent.save();
        assert!(!recent.writing(), "an unread list is never written over the stored one");
        recent.start_load(|| {});
        assert!(settle_load(&mut recent), "the merge asks for one write");
        assert_eq!(recent.entries(), [b.clone()]);
        recent.save();
        settle_write(&recent);
        assert_eq!(stored(&file), [b.clone()]);
        assert!(recent.record(&c));
        assert!(recent.forget(&b));
        assert!(!recent.forget(&a), "an unlisted path changes nothing once loaded");
        assert_eq!(recent.entries(), [c]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// APP-12: paths apply oldest first so the newest is on top, and a repeat of
    /// the top entry changes nothing, so nothing is written.
    #[test]
    fn recent_events_order_by_recency_and_repeats_change_nothing() {
        let (a, b) = (PathBuf::from("C:\\a.txt"), PathBuf::from("C:\\b.txt"));
        let mut recent = RecentFiles::default();
        assert!(recent.apply(&[a.clone(), b.clone(), a.clone()]));
        assert_eq!(recent.entries(), [a.clone(), b.clone()]);
        assert!(!recent.apply(std::slice::from_ref(&a)));
        assert!(!recent.apply(&[]));
        assert!(recent.apply(std::slice::from_ref(&b)));
        assert_eq!(recent.entries(), [b, a]);
    }
    /// BIZ-07: pinned entries lead the list, keep their place when opened,
    /// survive the cap, and come back pinned after a restart.
    #[test]
    fn recent_pins_stay_on_top_survive_the_cap_and_persist() {
        let dir = temp_dir("pins");
        let file = dir.join("recent.json");
        let path = |i: usize| PathBuf::from(format!("C:\\docs\\file{i}.txt"));
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        recent.start_load(|| {});
        settle_load(&mut recent);
        for i in 0..3 {
            assert!(recent.record(&path(i)));
        }
        assert_eq!(recent.set_pinned(&path(0), true), Ok(true));
        assert_eq!(recent.entries(), [path(0), path(2), path(1)]);
        assert_eq!(recent.pinned_len(), 1);
        // Opening a pinned file keeps its place; others go below the pins.
        assert!(!recent.record(&path(0)));
        assert!(recent.record(&path(1)));
        assert_eq!(recent.entries(), [path(0), path(1), path(2)]);
        // The cap only ever drops unpinned entries.
        for i in 3..30 {
            recent.record(&path(i));
        }
        assert_eq!(recent.entries().len(), RECENT_CAP);
        assert_eq!(recent.entries()[0], path(0));
        assert_eq!(recent.entries()[1], path(29));
        assert!(recent.is_pinned(&path(0)));
        // Pins fill at most two thirds of the list.
        for i in 20..29 {
            assert_eq!(recent.set_pinned(&path(i), true), Ok(true), "{i}");
        }
        assert_eq!(recent.pinned_len(), 10);
        assert!(recent.set_pinned(&path(29), true).is_err());
        assert_eq!(recent.entries().len(), RECENT_CAP);
        // Unpinning moves the entry to the top of the unpinned entries.
        assert_eq!(recent.set_pinned(&path(20), false), Ok(true));
        assert_eq!(recent.pinned_len(), 9);
        assert_eq!(recent.entries()[9], path(20));
        assert_eq!(recent.set_pinned(&path(20), false), Ok(false));
        // Removing a pinned entry keeps the pin count right.
        assert!(recent.forget(&path(0)));
        assert_eq!(recent.pinned_len(), 8);
        assert_eq!(recent.entries()[0], path(21));
        // Clear Unpinned keeps exactly the pins.
        assert!(recent.clear_unpinned());
        assert_eq!(recent.entries(), (21..29).map(path).collect::<Vec<_>>());
        assert!(!recent.clear_unpinned());
        recent.save();
        settle_write(&recent);
        let mut reloaded = RecentFiles::default();
        reloaded.configure(Some(file.clone()));
        reloaded.start_load(|| {});
        settle_load(&mut reloaded);
        assert_eq!(reloaded.entries(), recent.entries());
        assert_eq!(reloaded.pinned_len(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// BIZ-07 with APP-11: pins stored earlier stay pinned even when their file
    /// was opened before the stored list arrived, ahead of those files.
    #[test]
    fn stored_pins_merge_ahead_of_files_opened_before_the_list_loads() {
        let dir = temp_dir("pin-merge");
        let file = dir.join("recent.json");
        let (a, b, c) = (
            PathBuf::from("C:\\a.txt"),
            PathBuf::from("C:\\b.txt"),
            PathBuf::from("C:\\c.txt"),
        );
        write_recent(&file, 1, &[a.clone(), b.clone()]).unwrap();
        let mut recent = RecentFiles::default();
        recent.configure(Some(file.clone()));
        assert!(recent.apply(&[b.clone(), a.clone(), c.clone()]));
        recent.start_load(|| {});
        assert!(settle_load(&mut recent));
        assert_eq!(recent.entries(), [a.clone(), c.clone(), b.clone()]);
        assert_eq!(recent.pinned_len(), 1);
        // Clear Unpinned before a stored list arrives keeps only its pins.
        write_recent(&file, 1, &[a.clone(), b.clone()]).unwrap();
        let mut early = RecentFiles::default();
        early.configure(Some(file.clone()));
        early.record(&c);
        assert!(early.clear_unpinned());
        early.start_load(|| {});
        settle_load(&mut early);
        assert_eq!(early.entries(), [a]);
        assert_eq!(early.pinned_len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// BIZ-07: the pin count is a leading number that older builds skip; a
    /// list written by an older build reads back unpinned.
    #[test]
    fn recent_file_format_keeps_the_pin_count_readable_by_older_builds() {
        let (a, b) = (PathBuf::from("C:\\a \"x\".txt"), PathBuf::from("C:\\b.txt"));
        let text = encode_recent(1, &[a.clone(), b.clone()]);
        assert!(text.starts_with("[\n  1,\n"), "{text}");
        assert_eq!(
            decode_recent(&text),
            (
                1,
                vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()]
            )
        );
        assert_eq!(
            decode_recent("[\n  \"C:\\\\a.txt\"\n]\n"),
            (0, vec!["C:\\a.txt".to_owned()])
        );
        assert_eq!(decode_recent("[9, \"C:\\\\a.txt\"]"), (1, vec!["C:\\a.txt".to_owned()]));
        assert_eq!(decode_recent(&encode_recent(0, &[])), (0, Vec::new()));
        assert_eq!(decode_recent(&encode_recent(2, &[])), (0, Vec::new()));
    }
    /// BIZ-07: every listed Recent slot offers Pin (or Unpin) and Remove on
    /// right-click, and a chosen action pins or removes that slot's entry.
    #[test]
    fn recent_slot_right_click_pins_and_removes_that_entry() {
        use bareline_commands::{CommandContext, CommandId};
        let mut shell = super::super::accessibility::tests::headless_shell();
        let (a, b) = (PathBuf::from("C:\\a.txt"), PathBuf::from("D:\\work"));
        shell.shell_integration.recent_files.apply(&[a.clone()]);
        shell.shell_integration.recent_files.record(&PathBuf::from("C:\\b.txt"));
        shell.shell_integration.recent_folders.record(&b);
        let actions = shell.shell_integration.recent_item_actions().to_vec();
        let slots: Vec<_> = actions.iter().map(|(id, _)| id.0).collect();
        assert_eq!(slots, ["file.recent.0", "file.recent.1", "file.recent.folder.0"]);
        assert_eq!(actions[1].1[0], (RECENT_ACTION_PIN, "Pin to Top".to_owned()));
        assert_eq!(actions[1].1[1].0, RECENT_ACTION_REMOVE);
        shell.shell_recent_item_action("file.recent.1", RECENT_ACTION_PIN);
        assert_eq!(shell.shell_integration.recent_files.entries()[0], a);
        assert_eq!(shell.shell_integration.recent_files.pinned_len(), 1);
        let actions = shell.shell_integration.recent_item_actions();
        assert_eq!(actions[0].1[0], (RECENT_ACTION_PIN, "Unpin".to_owned()));
        let mut context = CommandContext::default();
        shell.shell_integration.annotate_context(&mut context, false, true);
        let label = |id| context.states.get(&CommandId(id)).and_then(|state| state.label.clone());
        assert_eq!(label("file.recent.0").as_deref(), Some("&1  a.txt  (pinned)"));
        assert_eq!(label("file.recent.1").as_deref(), Some("&2  b.txt"));
        assert_eq!(label("file.recent.folder.0").as_deref(), Some("&1  D:\\work"));
        assert!(context.states[&CommandId("file.recent.2")].hidden);
        assert!(!context.states.contains_key(&CommandId("file.recent.clearUnpinned")));
        shell.shell_recent_item_action("file.recent.folder.0", RECENT_ACTION_REMOVE);
        assert!(shell.shell_integration.recent_folders.entries().is_empty());
        shell.shell_recent_item_action("file.recent.0", RECENT_ACTION_REMOVE);
        assert_eq!(shell.shell_integration.recent_files.pinned_len(), 0);
        assert_eq!(shell.shell_integration.recent_files.entries().len(), 1);
    }
    /// APP-12: the list is replaced through a staged file, never left truncated
    /// or with the staging file behind.
    #[test]
    fn recent_list_is_replaced_atomically() {
        let dir = temp_dir("atomic");
        let file = dir.join("recent.json");
        let (a, b) = (PathBuf::from("C:\\a.txt"), PathBuf::from("C:\\b.txt"));
        write_recent(&file, 0, std::slice::from_ref(&a)).unwrap();
        write_recent(&file, 0, &[b.clone(), a.clone()]).unwrap();
        assert_eq!(stored(&file), [b, a]);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != "recent.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// APP-13: a data folder that refuses writes is reported, and recovery gets
    /// a local folder instead. A folder below a file stands in for read-only
    /// media: nothing can be created there.
    #[test]
    fn read_only_portable_folder_falls_back_to_a_local_recovery_folder() {
        let dir = temp_dir("portable");
        let writable = run_portable_probe(&dir.join("data"), Some(dir.join("fallback")));
        assert!(writable.writable.is_ok());
        assert_eq!(writable.fallback, None);
        assert!(!dir.join("fallback").exists());
        let blocker = dir.join("media");
        std::fs::write(&blocker, "not a folder").unwrap();
        let read_only = run_portable_probe(&blocker.join("data"), Some(dir.join("fallback")));
        assert!(read_only.writable.is_err());
        assert_eq!(read_only.fallback, Some(dir.join("fallback")));
        assert!(dir.join("fallback").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// APP-13: the shell stops saving the session, warns once, and points
    /// recovery at the fallback folder.
    #[test]
    fn read_only_decision_disables_session_saves_and_moves_recovery() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        let root = PathBuf::from("E:\\Bareline\\data");
        let fallback = PathBuf::from("C:\\Users\\u\\AppData\\Local\\Bareline\\portable-recovery");
        shell.shell_integration.portable_data = Some((root.clone(), Some(root.join("recovery"))));
        shell.session.configure(Some(root.join("session.json")), None, false);
        shell.portable_decided(PortableProbe {
            writable: Err("The media is write protected.".into()),
            fallback: Some(fallback.clone()),
        });
        assert!(!shell.session.persists(), "exit must not try to save the session");
        assert_eq!(shell.recovery_root, Some(fallback));
        assert_eq!(shell.toasts.persistent_len(), 1);
        assert!(shell.shell_integration.portable_data.is_none());
    }
    /// Each portable copy gets its own fallback folder; the same folder in a
    /// different case maps to the same one.
    #[test]
    fn portable_fallback_is_keyed_by_the_portable_folder() {
        let first = portable_recovery_key(std::path::Path::new(r"E:\Bareline\data"));
        assert_eq!(first.len(), 16);
        assert_eq!(first, portable_recovery_key(std::path::Path::new(r"e:\bareline\DATA")));
        assert_ne!(first, portable_recovery_key(std::path::Path::new(r"F:\Bareline\data")));
    }
}
