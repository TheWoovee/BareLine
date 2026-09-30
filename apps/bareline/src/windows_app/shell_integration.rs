// SPDX-License-Identifier: MPL-2.0
use super::*;
#[derive(Default)]
pub(super) struct ShellIntegrationRuntime {
    tray: Option<bareline_platform_windows::shell_integration::TrayIcon>,
    pub keep_in_tray: bool,
    recent: std::collections::BTreeSet<PathBuf>,
    pub recent_files: RecentFiles,
    pub portable: bool,
    /// The portable data folder and its recovery folder, until a worker decides
    /// after the first frame whether the folder accepts writes (APP-13).
    pub portable_data: Option<(PathBuf, Option<PathBuf>)>,
    portable_probe: Option<bareline_app::task::Task<PortableProbe>>,
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
/// The numbered Recent Files list. Nothing here touches the disk on the UI
/// thread: the stored list is read by a worker after the first frame (ADR-33,
/// APP-11) and every change is written atomically by a worker (APP-12).
#[derive(Default)]
pub(super) struct RecentFiles {
    path: Option<PathBuf>,
    entries: Vec<PathBuf>,
    load: Option<bareline_app::task::Task<Vec<PathBuf>>>,
    /// The stored list was read, or has nothing left to add.
    loaded: bool,
    /// A change made before the stored list arrived, written once it merges.
    deferred: bool,
    writer: std::sync::Arc<std::sync::Mutex<RecentWriter>>,
}
/// The newest unwritten list and whether a worker is writing; a burst of
/// changes costs one write of the latest list.
#[derive(Default)]
struct RecentWriter {
    pending: Option<Vec<PathBuf>>,
    running: bool,
}
impl RecentFiles {
    /// Remember where the list lives. Nothing is read yet (ADR-33).
    pub(super) fn configure(&mut self, path: Option<PathBuf>) {
        self.loaded = path.is_none();
        self.load = None;
        self.path = path;
    }
    pub(super) fn entries(&self) -> &[PathBuf] {
        &self.entries
    }
    /// Read the stored list on a worker; `notify` wakes the loop when it lands.
    pub(super) fn start_load(&mut self, notify: impl Fn() + Send + 'static) {
        if self.loaded || self.load.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        match bareline_app::task::spawn(notify, move |_| read_recent(&path)) {
            Ok(task) => self.load = Some(task),
            // Without a worker the stored list stays unread; recording still works.
            Err(_) => self.loaded = true,
        }
    }
    /// Merge the stored list once it arrives, behind anything recorded since
    /// startup. Returns whether the list changed.
    pub(super) fn poll(&mut self) -> bool {
        let Some(load) = &self.load else {
            return false;
        };
        let stored = match load.poll() {
            bareline_app::task::TaskPoll::Pending => return false,
            bareline_app::task::TaskPoll::Complete(stored) => stored,
            _ => Vec::new(),
        };
        self.load = None;
        self.loaded = true;
        let before = self.entries.clone();
        for path in stored {
            if !self.entries.contains(&path) && self.entries.len() < RECENT_CAP {
                self.entries.push(path);
            }
        }
        self.entries != before || std::mem::take(&mut self.deferred)
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
        // A stored list still being read must not come back after a clear.
        self.load = None;
        self.loaded = true;
        self.save();
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
            writer.pending = Some(self.entries.clone());
            if writer.running {
                return;
            }
            writer.running = true;
        }
        let writer = self.writer.clone();
        let job = move || {
            loop {
                let entries = {
                    let mut state = writer.lock().unwrap_or_else(|error| error.into_inner());
                    match state.pending.take() {
                        Some(entries) => entries,
                        None => {
                            state.running = false;
                            return;
                        }
                    }
                };
                if let Err(error) = write_recent(&path, &entries) {
                    eprintln!("event=recent_files_write_failed kind={:?}", error.kind());
                }
            }
        };
        if bareline_app::task::execute(job).is_err() {
            // The next change tries again with the newest list.
            self.writer.lock().unwrap_or_else(|error| error.into_inner()).running = false;
        }
    }
    /// Whether a list is waiting to be written or being written.
    #[cfg(test)]
    fn writing(&self) -> bool {
        let writer = self.writer.lock().unwrap_or_else(|error| error.into_inner());
        writer.running || writer.pending.is_some()
    }
}
fn read_recent(path: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_to_string(path)
        .map(|text| {
            decode_recent(&text)
                .into_iter()
                .map(PathBuf::from)
                .take(RECENT_CAP)
                .collect()
        })
        .unwrap_or_default()
}
/// Stage beside the target and rename over it, so a crash or a full disk never
/// leaves a truncated `recent.json`.
fn write_recent(path: &std::path::Path, entries: &[PathBuf]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stage = path.with_file_name(format!(".recent-{}.tmp", std::process::id()));
    let result = std::fs::File::create(&stage)
        .and_then(|mut file| {
            file.write_all(encode_recent(entries).as_bytes())?;
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
    /// Start reading the stored Recent Files list; called after the first frame.
    pub(super) fn shell_recent_start(&mut self) {
        let wake = self.wake.clone();
        self.shell_integration
            .recent_files
            .start_load(move || wake(Wake::One(Source::ShellRecent)));
    }
    pub(super) fn shell_recent_pump(&mut self) {
        let mut changed = self.shell_integration.recent_files.poll();
        // Only opens, saves and closes reorder the list; open tabs are never
        // re-recorded on a wake (APP-12).
        let paths = self
            .workspace
            .as_mut()
            .map(|workspace| workspace.take_recent_events())
            .unwrap_or_default();
        changed |= self.shell_integration.recent_files.apply(&paths);
        for path in paths {
            // Our own numbered Recent Files list is persisted locally and works
            // in portable mode; the Windows shell MRU is only touched when installed.
            if !self.shell_integration.portable
                && self.shell_integration.recent.len() < 256
                && self.shell_integration.recent.insert(path.clone())
            {
                bareline_platform_windows::shell_integration::add_recent(&path, false);
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
        read_recent(file)
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
        write_recent(&file, &[a.clone(), b.clone()]).unwrap();
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
    /// APP-12: the list is replaced through a staged file, never left truncated
    /// or with the staging file behind.
    #[test]
    fn recent_list_is_replaced_atomically() {
        let dir = temp_dir("atomic");
        let file = dir.join("recent.json");
        let (a, b) = (PathBuf::from("C:\\a.txt"), PathBuf::from("C:\\b.txt"));
        write_recent(&file, std::slice::from_ref(&a)).unwrap();
        write_recent(&file, &[b.clone(), a.clone()]).unwrap();
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
