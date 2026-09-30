// SPDX-License-Identifier: MPL-2.0
//! Native composition only. Bounded background metadata reconciliation never replaces buffers.
use super::*;
use bareline_platform::{FileIdentity, LocalFileSystem, PathOrigin, PathTrustProvider};
use bareline_platform_windows::{WindowsFileSystem, WindowsPathTrustProvider, WindowsWatchService};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::mpsc::{self, Receiver},
};

type Registration = Result<Option<WindowsWatchService>, String>;
type Checked = (PathBuf, FileIdentity, Result<bool, String>);
/// A drawn banner, its message and the full path of its document: the
/// accessible description and hover tooltip carry the path (UI-02).
pub(super) type BannerNote = (bareline_renderer::Rect, String, PathBuf);
/// Bands a view reserves above its text for a follow banner (34 px) or an
/// external-change banner (62 px), each with 4 px margins, so neither covers
/// the tab strip or the first text line (UI-02).
pub(super) const FOLLOW_BANNER_BAND: f32 = 42.0;
pub(super) const CONFLICT_BANNER_BAND: f32 = 70.0;
/// First accessibility id of the banner status regions (one per pane).
pub(super) const WATCH_BANNER_ID: u64 = 90_000_060;
/// The `height` band a view of document `index` at `bounds` reserved above its
/// text (its surface's `top_inset`): under the tab strip and any Find bar,
/// above a binary notice. Zero height when the view shows no banner.
pub(super) fn banner_band_rect(
    w: &bareline_app::workspace::Workspace,
    index: usize,
    bounds: bareline_renderer::Rect,
    top_inset: f32,
    height: f32,
) -> bareline_renderer::Rect {
    let notice = if w.binary_warning_pending(index) {
        bareline_app::encoding::BINARY_NOTICE_HEIGHT
    } else {
        0.0
    };
    bareline_ui::rect(
        bounds.x,
        bounds.y + bareline_ui::TAB_HEIGHT + top_inset - notice - height,
        bounds.width,
        height,
    )
}
/// File name for a banner message; the full path goes to the tooltip.
fn banner_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
/// While the pointer rests on a banner's message, show its full path just
/// below the banner (whose bottom edge is `below`), clear of its actions.
fn path_tooltip(
    message: bareline_renderer::Rect,
    below: f32,
    path: &std::path::Path,
    pointer: Point,
    ops: &mut Vec<bareline_renderer::DrawOp>,
) {
    use bareline_renderer::DrawOp;
    use bareline_ui::{BORDER, ELEVATED, TEXT, rect, text};
    if !message.contains(pointer) {
        return;
    }
    let full = path.display().to_string();
    let tip = rect(message.x, below + 2.0, full.chars().count() as f32 * 7.0 + 16.0, 22.0);
    ops.push(DrawOp::Fill(tip, ELEVATED));
    ops.push(DrawOp::Stroke(tip, BORDER, 1.0));
    ops.push(DrawOp::PushClip(tip));
    text(ops, tip.x + 8.0, tip.y + 3.0, full, 12.0, TEXT);
    ops.push(DrawOp::PopClip);
}
/// Draws a paged follow banner in `band` (see `banner_band_rect`).
pub(super) fn draw_banner(
    editor: &bareline_app::workspace::WorkspaceEditor,
    band: bareline_renderer::Rect,
    pointer: Point,
    banners: &mut Vec<BannerNote>,
    ops: &mut Vec<bareline_renderer::DrawOp>,
) -> Vec<(bareline_renderer::Rect, bareline_commands::CommandId)> {
    use bareline_renderer::DrawOp;
    use bareline_ui::{ACCENT, CHROME, TEXT, rect, text};
    let bareline_app::workspace::WorkspaceEditor::Paged(editor) = editor else {
        return Vec::new();
    };
    let Some((paused, changed)) = editor.follow_status() else {
        return Vec::new();
    };
    let changed = changed || editor.source_changed();
    let banner = rect(band.x + 8.0, band.y + 4.0, (band.width - 16.0).max(0.0), 34.0);
    ops.push(DrawOp::FillRounded(banner, CHROME, 4.0));
    ops.push(DrawOp::StrokeRounded(banner, ACCENT, 4.0, 1.0));
    let actions = if changed {
        vec![
            ("Reopen and follow", "file.monitor.reopen"),
            ("Unlock captured content", "file.monitor.unlock"),
        ]
    } else {
        vec![
            (
                if paused { "Resume ↓" } else { "Pause" },
                if paused {
                    "file.monitor.resume"
                } else {
                    "file.monitor.pause"
                },
            ),
            ("Unlock to edit", "file.monitor.unlock"),
        ]
    };
    let action_width = (if changed { 175.0_f32 } else { 135.0_f32 }).min(((banner.width - 16.0) / 2.0).max(0.0));
    let actions_x = (banner.x + banner.width - action_width * 2.0 - 8.0).max(banner.x + 8.0);
    let message = rect(
        banner.x + 12.0,
        banner.y,
        (actions_x - banner.x - 20.0).max(0.0),
        banner.height,
    );
    ops.push(DrawOp::PushClip(message));
    let editor_path = editor.path();
    // Lead with the file name so it survives a narrow banner; the full path is
    // the tooltip and accessible description (UI-02).
    let name = banner_name(&editor_path);
    let label = if changed {
        format!("{name} · Source changed")
    } else {
        format!(
            "{name} · {}",
            if paused {
                "Following paused (scrolled up)"
            } else {
                "Following new content"
            }
        )
    };
    text(ops, banner.x + 12.0, banner.y + 8.0, label.clone(), 14.0, TEXT);
    ops.push(DrawOp::PopClip);
    path_tooltip(message, banner.y + banner.height, &editor_path, pointer, ops);
    banners.push((banner, label, editor_path.to_path_buf()));
    let mut hits = Vec::new();
    for (index, (label, command)) in actions.into_iter().enumerate() {
        let bounds = rect(
            actions_x + index as f32 * action_width,
            banner.y,
            action_width,
            banner.height,
        );
        ops.push(DrawOp::PushClip(bounds));
        text(ops, bounds.x + 6.0, bounds.y + 8.0, label, 14.0, ACCENT);
        ops.push(DrawOp::PopClip);
        hits.push((bounds, bareline_commands::CommandId(command)));
    }
    hits
}
#[derive(Default)]
pub(super) struct WatchRuntime {
    pub(super) hits: Vec<(bareline_renderer::Rect, u32, usize, bareline_commands::CommandId)>,
    /// Banners drawn in the last frame, for their accessible description.
    pub(super) banners: Vec<BannerNote>,
    service: Option<WindowsWatchService>,
    setup: Option<Receiver<Registration>>,
    registered: Vec<PathBuf>,
    desired: Vec<PathBuf>,
    queue: VecDeque<(PathBuf, FileIdentity)>,
    checking: Option<Receiver<Vec<Checked>>>,
    conflicts: BTreeSet<PathBuf>,
    observed_identities: std::collections::BTreeMap<PathBuf, FileIdentity>,
    requested: bool,
    auto_reload_clean: bool,
    reopen_follow: BTreeSet<PathBuf>,
    reopen_pane: std::collections::BTreeMap<PathBuf, u32>,
    remote_wake: Option<Receiver<()>>,
    remote_follow:
        std::collections::BTreeMap<PathBuf, (bareline_platform::RemoteReadGrant, std::sync::Arc<dyn LocalFileSystem>)>,
}
pub(super) fn commands() -> Vec<bareline_commands::CommandSpec> {
    [
        ("file.remote.open", "Open Remote File with Permission…"),
        ("file.remote.reload", "Reload Remote File with Permission…"),
        ("file.remote.follow", "Follow Remote File with Permission…"),
        (
            "file.external.auto_reload",
            "Automatically Reload Clean Local Files (This Session)",
        ),
        ("file.external.check", "Check for External Changes"),
        ("file.external.keep", "Keep Current Buffer"),
        ("file.external.reload", "Reload External Changes"),
        ("file.monitor.start", "Follow New Content"),
        ("file.monitor.pause", "Pause Following Scroll"),
        ("file.monitor.resume", "Resume Following"),
        ("file.monitor.reopen", "Reopen and Follow"),
        ("file.monitor.unlock", "Unlock to Edit (Stop Monitoring)"),
    ]
    .into_iter()
    .map(|(id, title)| bareline_commands::CommandSpec {
        id: bareline_commands::CommandId(id),
        title,
        category: "File",
        shortcut: "",
        action: Action::Contributed(bareline_commands::CommandId(id)),
    })
    .collect()
}
impl Shell {
    pub(super) fn watch_start_follow(&mut self, index: usize) -> Result<(), String> {
        if self.views.pane() == 1 {
            if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = &mut self.views.secondary {
                return editor.start_follow(std::sync::Arc::new(WindowsFileSystem));
            }
        }
        self.watch_start_follow_workspace(index, self.views.pane())
    }
    pub(super) fn watch_start_follow_document(&mut self, document: (u64, u64)) -> Result<(), String> {
        let index = launch_follow_index(self.workspace.as_ref().ok_or("Workspace unavailable")?, document)
            .ok_or("Document unavailable")?;
        self.watch_start_follow_workspace(index, 0)
    }
    fn watch_start_follow_workspace(&mut self, index: usize, reopen_pane: u32) -> Result<(), String> {
        let w = self.workspace.as_mut().ok_or("Workspace unavailable")?;
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = w.editors.get_mut(index) {
            return editor.start_follow(std::sync::Arc::new(WindowsFileSystem));
        }
        let editor = w.editors.get_mut(index).ok_or("Document unavailable")?;
        if editor.dirty() {
            return Err("Save or discard edits before monitoring.".into());
        }
        let path = w.path(index).ok_or("Save this file before monitoring")?.to_owned();
        let was_read_only = w.editors[index].viewport().user_read_only;
        w.editors[index].viewport_mut().user_read_only = false;
        let threshold = w.resident_max_bytes;
        w.resident_max_bytes = 0;
        let result = w.reload(index, false);
        w.resident_max_bytes = threshold;
        w.editors[index].viewport_mut().user_read_only = was_read_only;
        result?;
        self.watch.reopen_pane.insert(path.clone(), reopen_pane);
        self.watch.reopen_follow.insert(path);
        Ok(())
    }
    pub(super) fn watch_sync(&mut self) {
        let mut paths = self.workspace_watch_roots();
        if let Some(w) = &self.workspace {
            for index in 0..w.editors.len() {
                if let Some(path) = w.path(index)
                    && let Some(parent) = path.parent()
                {
                    if !self.watch.remote_follow.contains_key(path) && !is_network_path(path) {
                        paths.push(parent.to_owned());
                    }
                }
            }
        }
        paths.retain(|path| !is_network_path(path));
        paths.sort();
        paths.dedup();
        if paths.len() > 63 {
            paths.truncate(63);
            if let Some(w) = &mut self.workspace {
                w.message = Some(
                    "Watching 63 directories. Additional directories reconcile on focus or Check for External Changes."
                        .into(),
                );
            }
        }
        self.watch.desired = paths;
        if self.watch.setup.is_some() || self.watch.desired == self.watch.registered {
            return;
        }
        let desired = self.watch.desired.clone();
        let previous = self.watch.service.take();
        let notify = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-watch-setup".into())
            .spawn(move || {
                drop(previous);
                let result = if desired.is_empty() {
                    Ok(None)
                } else {
                    WindowsWatchService::start_notifying(desired, notify.clone())
                        .map(Some)
                        .map_err(|e| e.to_string())
                };
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.watch.registered = self.watch.desired.clone();
                self.watch.setup = Some(rx);
            }
            Err(e) => {
                self.toasts.push_typed(
                    "watch-setup",
                    toast::next_revision(),
                    bareline_ui::theme::ToastLevel::Error,
                    toast::NotificationKind::Outcome,
                    "File watching unavailable.",
                    Some(e.to_string()),
                    None,
                    toast::NotificationLifetime::Persistent,
                    Instant::now(),
                );
            }
        }
    }
    pub(super) fn watch_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "file.remote.open" | "file.remote.reload" | "file.remote.follow" => {
                self.watch_remote_request(el, id);
                true
            }
            "file.monitor.start" => {
                if let Err(error) = self.watch_start_follow(self.app.active) {
                    let document = self
                        .workspace
                        .as_ref()
                        .and_then(|w| w.editors.get(self.app.active))
                        .map(|editor| editor.snapshot().identity_token());
                    self.toasts.push_typed(
                        "watch-follow-start",
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "File monitoring could not start.",
                        Some(error),
                        document,
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
                true
            }
            "file.monitor.pause" | "file.monitor.resume" => {
                if let Some(w) = &mut self.workspace
                    && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) =
                        self.views.active_workspace_editor_mut(w, self.app.active)
                {
                    editor.set_follow_paused(id == "file.monitor.pause");
                    if id == "file.monitor.resume" {
                        let _ = editor.request_viewport(bareline_document::TextOffset(
                            editor.snapshot().len().saturating_sub(32768),
                        ));
                    }
                }
                true
            }
            "file.monitor.unlock" => {
                let confirmed = self.platform.as_ref().is_some_and(|p| p.confirm_stop_monitoring());
                if confirmed
                    && let Some(w) = &mut self.workspace
                    && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) =
                        self.views.active_workspace_editor_mut(w, self.app.active)
                {
                    if let Err(error) = editor.unlock_follow(true) {
                        let document = editor.snapshot().identity_token();
                        self.toasts.push_typed(
                            format!("watch-unlock-{}", document.0),
                            toast::next_revision(),
                            bareline_ui::theme::ToastLevel::Error,
                            toast::NotificationKind::Outcome,
                            "Monitoring could not be stopped.",
                            Some(error),
                            Some(document),
                            toast::NotificationLifetime::Persistent,
                            Instant::now(),
                        );
                    }
                }
                true
            }
            "file.monitor.reopen" => {
                if self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(self.app.active))
                    .is_some_and(|path| self.watch.remote_follow.contains_key(path) || is_network_path(path))
                {
                    self.watch_remote_request(el, "file.remote.follow");
                    return true;
                }
                if let Some(w) = &mut self.workspace
                    && let Some(editor) = w.editors.get_mut(self.app.active)
                {
                    let was_read_only = editor.viewport().user_read_only;
                    editor.viewport_mut().user_read_only = false;
                    let path = w.path(self.app.active).map(|p| p.to_owned());
                    let result = w.reload(self.app.active, false);
                    w.editors[self.app.active].viewport_mut().user_read_only = was_read_only;
                    match result {
                        Err(error) => {
                            let document = w.editors[self.app.active].snapshot().identity_token();
                            self.toasts.push_typed(
                                format!("watch-reopen-{}", document.0),
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "Monitored file could not be reopened.",
                                Some(error),
                                Some(document),
                                toast::NotificationLifetime::Persistent,
                                Instant::now(),
                            );
                        }
                        Ok(()) => {
                            if let Some(path) = path {
                                self.watch.reopen_pane.insert(path.clone(), self.views.pane());
                                self.watch.reopen_follow.insert(path);
                            }
                        }
                    }
                }
                true
            }
            "file.external.auto_reload" => {
                self.watch.auto_reload_clean = !self.watch.auto_reload_clean;
                true
            }
            "file.external.check" => {
                self.watch.requested = true;
                if self.watch.service.is_none() && self.watch.setup.is_none() {
                    self.watch.registered.clear();
                }
                (self.notify)();
                true
            }
            "file.external.reload" => {
                let active = self.app.active;
                let dirty = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.editors.get(active))
                    .is_some_and(|e| e.dirty());
                let name = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(active))
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "document".into());
                let confirmed = !dirty
                    || self
                        .platform
                        .as_ref()
                        .is_some_and(|p| p.confirm_discard_and_reload(&name));
                if confirmed
                    && let Some(w) = &mut self.workspace
                    && let Err(e) = w.reload(active, dirty)
                {
                    let document = w.editors.get(active).map(|editor| editor.snapshot().identity_token());
                    self.toasts.push_typed(
                        "watch-external-reload",
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "The externally changed file could not be reloaded.",
                        Some(e),
                        document,
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
                true
            }
            "file.external.keep" => {
                if let Some(w) = &mut self.workspace {
                    if let Some(path) = w.path(self.app.active) {
                        self.watch.conflicts.remove(path);
                        self.toasts.resolve(&conflict_notification_id(path));
                    }
                    w.message = Some(
                        "Current buffer kept. Save will still check the disk version; use Save As to preserve both."
                            .into(),
                    );
                }
                true
            }
            _ => false,
        }
    }
    fn apply_watch_check_results(&mut self, results: Vec<Checked>) {
        let page_open = self.settings.controller.open || self.extensions.open;
        if let Some(w) = &mut self.workspace {
            for (path, expected, result) in results {
                if let Some(index) = (0..w.editors.len()).find(|&i| w.path(i) == Some(path.as_path())) {
                    if w.editors[index].busy()
                        || w.fingerprint(index).map(|fingerprint| fingerprint.identity) != Some(expected)
                    {
                        self.watch.requested = true;
                        continue;
                    }
                    if result == Ok(false) {
                        self.watch.conflicts.remove(&path);
                        self.toasts.resolve(&conflict_notification_id(&path));
                    }
                }
                let index = (0..w.editors.len()).find(|&i| {
                    w.path(i) == Some(path.as_path()) && w.fingerprint(i).is_some_and(|f| f.identity == expected)
                });
                if let Some(index) = index
                    && !matches!(&w.editors[index], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())
                    && (result.as_ref().is_err() || result == Ok(true))
                {
                    if should_auto_reload(
                        self.watch.auto_reload_clean,
                        w.editors[index].dirty(),
                        w.editors[index].read_only(),
                        w.editors[index].busy(),
                        result == Ok(true),
                        is_network_path(&path),
                    ) && w.reload(index, false).is_ok()
                    {
                        continue;
                    }
                    self.watch.conflicts.insert(path.clone());
                    // A shown document's banner already carries this event; a
                    // toast would duplicate it (UI-02: one notification).
                    if document_shown(&self.views, self.app.active, page_open, w, index) {
                        continue;
                    }
                    let document = w.editors[index].snapshot().identity_token();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    let message = if w.editors[index].dirty() {
                        format!("External change detected: {name}. Your edits are preserved.")
                    } else {
                        format!("External change or unavailable file: {name}. Current bytes are preserved.")
                    };
                    self.toasts.push_typed(
                        conflict_notification_id(&path),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Warning,
                        toast::NotificationKind::Outcome,
                        message,
                        Some(format!(
                            "Path: {}\n\nChoose Keep Current Buffer, Save As, or explicitly reload.",
                            path.display()
                        )),
                        Some(document),
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
            }
        }
    }
    pub(super) fn watch_pump(&mut self, _: &ActiveEventLoop) {
        // Save acknowledgments can arrive after the filesystem event batch. Recheck
        // against the new identity even when no further OS event will wake watching.
        // Retain conflicts until this fresh check actually verifies the disk bytes.
        let identities: std::collections::BTreeMap<_, _> = self
            .workspace
            .as_ref()
            .map(|workspace| {
                (0..workspace.editors.len())
                    .filter_map(|index| {
                        Some((
                            workspace.path(index)?.to_owned(),
                            workspace.fingerprint(index)?.identity,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if identities != self.watch.observed_identities {
            self.watch.observed_identities = identities;
            self.watch.requested = true;
        }
        let mut open: BTreeSet<_> = self.workspace.as_ref().map(|w|(0..w.editors.len()).filter(|&i|w.path(i).is_some_and(|p|w.path_loading(p)||self.watch.reopen_follow.contains(p))||w.document_busy(i)||matches!(&w.editors[i],bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())).filter_map(|i|w.path(i).map(|p|p.to_owned())).collect()).unwrap_or_default();
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = &self.views.secondary {
            if editor.follow_status().is_some() || editor.busy() {
                open.insert(editor.path());
            }
        }
        self.watch.remote_follow.retain(|path, (grant, _)| {
            if !open.contains(path) {
                grant.revoke();
                false
            } else {
                true
            }
        });
        if self.watch.remote_wake.as_ref().is_some_and(|rx| rx.try_recv().is_ok()) {
            self.watch.remote_wake = None;
            self.watch.requested = true;
        }
        if !self.watch.remote_follow.is_empty() && self.watch.remote_wake.is_none() {
            let (tx, rx) = mpsc::sync_channel(1);
            let notify = self.notify.clone();
            if std::thread::Builder::new()
                .name("bareline-remote-follow-wake".into())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let _ = tx.send(());
                    notify();
                })
                .is_ok()
            {
                self.watch.remote_wake = Some(rx);
            }
        }
        let pending: Vec<_> = self.watch.reopen_follow.iter().cloned().collect();
        for path in pending {
            if !self
                .workspace
                .as_ref()
                .is_some_and(|w| (0..w.editors.len()).any(|i| w.path(i) == Some(path.as_path())))
            {
                self.watch.reopen_follow.remove(&path);
                self.watch.reopen_pane.remove(&path);
                continue;
            }
            let index = self.workspace.as_ref().and_then(|w| (0..w.editors.len()).find(|&i| w.path(i) == Some(path.as_path()) && matches!(&w.editors[i], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_none() && !e.busy())));
            if let Some(index) = index {
                let provider = self
                    .watch
                    .remote_follow
                    .get(&path)
                    .map(|(_, fs)| fs.clone())
                    .unwrap_or_else(|| std::sync::Arc::new(WindowsFileSystem));
                let result = if self.watch.reopen_pane.get(&path) == Some(&1) {
                    if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = &mut self.views.secondary {
                        if editor.path() != path || editor.busy() || editor.follow_status().is_some() {
                            continue;
                        }
                        editor.start_follow(provider)
                    } else {
                        continue;
                    }
                } else if let Some((_, fs)) = self.watch.remote_follow.get(&path) {
                    if let Some(w) = &mut self.workspace
                        && let bareline_app::workspace::WorkspaceEditor::Paged(editor) = &mut w.editors[index]
                    {
                        editor.start_follow(fs.clone())
                    } else {
                        Err("Paged follow view unavailable".into())
                    }
                } else {
                    if let Some(w) = &mut self.workspace
                        && let bareline_app::workspace::WorkspaceEditor::Paged(editor) = &mut w.editors[index]
                    {
                        editor.start_follow(provider)
                    } else {
                        Err("Paged follow view unavailable".into())
                    }
                };
                if result.is_ok() {
                    self.watch.reopen_follow.remove(&path);
                    self.watch.reopen_pane.remove(&path);
                } else if let Err(error) = result {
                    self.watch.reopen_follow.remove(&path);
                    self.watch.reopen_pane.remove(&path);
                    if let Some((grant, _)) = self.watch.remote_follow.remove(&path) {
                        grant.revoke();
                    }
                    self.toasts.push_typed(
                        format!("watch-follow-reopen:{}", path.display()),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "File monitoring could not resume.",
                        Some(error),
                        None,
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
            }
        }
        if let Some(rx) = &self.watch.setup {
            match rx.try_recv() {
                Ok(result) => {
                    self.watch.setup = None;
                    match result {
                        Ok(service) => {
                            self.watch.service = service;
                            self.watch.requested = true;
                        }
                        Err(e) => {
                            self.toasts.push_typed(
                                "watch-setup",
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "File watching unavailable.",
                                Some(format!("{e}. Check for External Changes remains available.")),
                                None,
                                toast::NotificationLifetime::Persistent,
                                Instant::now(),
                            );
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.setup = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut events = Vec::new();
        if let Some(service) = &self.watch.service {
            for _ in 0..256 {
                let Some(event) = service.try_recv() else {
                    break;
                };
                events.push(event);
                self.watch.requested = true;
            }
        }
        for event in events {
            self.workspace_watch_event(&event);
        }
        if let Some(w) = &mut self.workspace {
            for editor in &mut w.editors {
                if let bareline_app::workspace::WorkspaceEditor::Paged(editor) = editor {
                    let editor_path = editor.path();
                    if let Err(error) = editor.follow_tick(
                        self.watch
                            .remote_follow
                            .get(&editor_path)
                            .map(|(_, fs)| fs.clone())
                            .unwrap_or_else(|| std::sync::Arc::new(WindowsFileSystem)),
                        self.watch.requested,
                    ) {
                        editor.error = Some(error);
                    }
                }
            }
        }
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = &mut self.views.secondary {
            let editor_path = editor.path();
            let platform = self
                .watch
                .remote_follow
                .get(&editor_path)
                .map(|(_, fs)| fs.clone())
                .unwrap_or_else(|| std::sync::Arc::new(WindowsFileSystem));
            if let Err(error) = editor.follow_tick(platform, self.watch.requested) {
                editor.error = Some(error);
            }
        }
        if let Some(rx) = &self.watch.checking {
            match rx.try_recv() {
                Ok(results) => {
                    self.watch.checking = None;
                    self.apply_watch_check_results(results);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.checking = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.watch.requested && self.watch.queue.is_empty() && self.watch.checking.is_none() {
            self.watch.requested = false;
            if let Some(w) = &self.workspace {
                for i in 0..w.editors.len() {
                    if w.editors[i].busy() {
                        self.watch.requested = true;
                        continue;
                    }
                    if let (Some(path), Some(f)) = (w.path(i), w.fingerprint(i)) {
                        self.watch.queue.push_back((path.to_owned(), f.identity));
                    }
                }
            }
        }
        if self.watch.checking.is_none() && !self.watch.queue.is_empty() {
            let batch: Vec<_> = (0..16).filter_map(|_| self.watch.queue.pop_front()).collect();
            let notify = self.notify.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            match std::thread::Builder::new()
                .name("bareline-watch-check".into())
                .spawn(move || {
                    let results = batch
                        .into_iter()
                        .map(|(path, expected)| {
                            let result = WindowsPathTrustProvider
                                .open_read(&path, PathOrigin::User)
                                .and_then(|opened| WindowsFileSystem.identity(&opened.file))
                                .map(|actual| actual != expected)
                                .map_err(|e| e.to_string());
                            (path, expected, result)
                        })
                        .collect();
                    let _ = tx.send(results);
                    notify();
                }) {
                Ok(_) => self.watch.checking = Some(rx),
                Err(e) => {
                    self.toasts.push_typed(
                        "watch-external-check",
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "External-change check unavailable.",
                        Some(e.to_string()),
                        None,
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
            }
        }
        self.watch_sync();
    }
}

impl Shell {
    pub(super) fn watch_focus_check(&mut self) {
        self.watch.requested = true;
        (self.notify)();
    }
    pub(super) fn watch_annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        context.states.insert(
            CommandId("file.external.auto_reload"),
            CommandState {
                checked: self.watch.auto_reload_clean,
                ..Default::default()
            },
        );
        let editor = self
            .workspace
            .as_ref()
            .and_then(|w| self.views.active_workspace_editor(w, self.app.active));
        let path = self.workspace.as_ref().and_then(|w| w.path(self.app.active));
        let source_changed =
            matches!(editor,Some(bareline_app::workspace::WorkspaceEditor::Paged(e)) if e.source_changed());
        let follow = editor.and_then(|e| {
            if let bareline_app::workspace::WorkspaceEditor::Paged(e) = e {
                e.follow_status()
                    .map(|(paused, changed)| (paused, changed || e.source_changed()))
            } else {
                None
            }
        });
        let busy = editor.is_some_and(|e| e.busy());
        for (id, enabled, reason) in [
            (
                "file.remote.reload",
                path.is_some() && !busy,
                "Open a document and wait for current work",
            ),
            (
                "file.remote.follow",
                path.is_some()
                    && !busy
                    && (follow.is_none() || follow.is_some_and(|(_, changed)| changed))
                    && !editor.is_some_and(|e| e.dirty()),
                "Save edits and wait before following",
            ),
            (
                "file.external.keep",
                path.is_some_and(|p| self.watch.conflicts.contains(p)) || source_changed,
                "No external change to keep",
            ),
            (
                "file.external.reload",
                path.is_some() && !busy,
                "Save this document first or wait for its current operation",
            ),
            (
                "file.monitor.start",
                path.is_some() && !busy && follow.is_none() && !editor.is_some_and(|e| e.dirty()),
                "Save edits and wait for current work before following",
            ),
            (
                "file.monitor.pause",
                follow == Some((false, false)),
                "No active following scroll to pause",
            ),
            (
                "file.monitor.resume",
                follow == Some((true, false)),
                "Following is not paused, or the source changed",
            ),
            (
                "file.monitor.reopen",
                follow.is_some_and(|(_, changed)| changed) && !busy,
                "The monitored source has not changed, or work is pending",
            ),
            (
                "file.monitor.unlock",
                follow.is_some() && !busy,
                "No monitored generation ready to unlock",
            ),
        ] {
            if !enabled {
                context.states.insert(CommandId(id), CommandState::disabled(reason));
            }
        }
    }
}
impl Shell {
    /// Each banner drawn last frame as a status region: the message leads with
    /// the file name and the full path is its value, the accessible description
    /// matching the hover tooltip (UI-02).
    pub(super) fn watch_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        self.watch
            .banners
            .iter()
            .take(2)
            .enumerate()
            .map(|(slot, (bounds, message, path))| AccessibilityNode {
                id: WATCH_BANNER_ID + slot as u64,
                parent: 1,
                role: AccessibilityRole::Status,
                name: message.clone(),
                value: Some(path.display().to_string()),
                bounds: [
                    bounds.x as f64,
                    bounds.y as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                ],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            })
            .collect()
    }
}
impl WatchRuntime {
    /// Height of the banner band `editor` (document `index`, or a split view of
    /// it) needs above its text: follow banner, external-change banner or none.
    pub(super) fn banner_band(
        &self,
        w: &bareline_app::workspace::Workspace,
        index: usize,
        editor: &bareline_app::workspace::WorkspaceEditor,
    ) -> f32 {
        use bareline_app::workspace::WorkspaceEditor;
        if matches!(editor, WorkspaceEditor::Paged(e) if e.follow_status().is_some()) {
            return FOLLOW_BANNER_BAND;
        }
        let source_changed = matches!(editor, WorkspaceEditor::Paged(e) if e.source_changed());
        if w.path(index)
            .is_some_and(|path| self.conflicts.contains(path) || source_changed)
        {
            CONFLICT_BANNER_BAND
        } else {
            0.0
        }
    }
    /// Bands for every document with a banner, by document id, published to
    /// the workspace before layout so views push their text down (UI-02). The
    /// id, not the revision, keys them: a split pane's sync can bump the
    /// revision after they are published.
    pub(super) fn banner_bands(&self, w: &bareline_app::workspace::Workspace) -> std::collections::BTreeMap<u64, f32> {
        let mut bands = std::collections::BTreeMap::new();
        for (index, editor) in w.editors.iter().enumerate() {
            let band = self.banner_band(w, index, editor);
            if band > 0.0 {
                bands.insert(editor.document_identity().0, band);
            }
        }
        bands
    }
    /// The band a split pane's own view reserves: it can follow on its own, so
    /// it reserves only the banner it will draw, never the other pane's (UI-02).
    /// The view maps to its document by its loaded tab, as a paged clone has a
    /// document identity of its own.
    pub(super) fn view_banner_band(
        &self,
        w: &bareline_app::workspace::Workspace,
        views: &super::views::ViewsRuntime,
    ) -> f32 {
        views
            .secondary
            .as_ref()
            .zip(views.secondary_index(w))
            .map_or(0.0, |(editor, index)| self.banner_band(w, index, editor))
    }
    /// A shown document's banner is its external-change notification, so the
    /// toast raised while it was in the background retires (UI-02: one
    /// notification per event).
    pub(super) fn retire_shown_conflict_notices(
        &self,
        w: &bareline_app::workspace::Workspace,
        views: &super::views::ViewsRuntime,
        active: usize,
        page_open: bool,
        toasts: &mut toast::ToastStack,
    ) {
        for path in &self.conflicts {
            if let Some(index) = (0..w.editors.len()).find(|&index| w.path(index) == Some(path.as_path()))
                && document_shown(views, active, page_open, w, index)
            {
                toasts.resolve(&conflict_notification_id(path));
            }
        }
    }
    /// Draws the follow or external-change banner of `editor`, a view of
    /// document `index` (the document itself or a split pane's view), in `band`.
    pub(super) fn draw_banner(
        &mut self,
        w: &bareline_app::workspace::Workspace,
        index: usize,
        editor: &bareline_app::workspace::WorkspaceEditor,
        band: bareline_renderer::Rect,
        pointer: Point,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Vec<(bareline_renderer::Rect, bareline_commands::CommandId)> {
        let follow = draw_banner(editor, band, pointer, &mut self.banners, ops);
        if !follow.is_empty() {
            return follow;
        }
        let source_changed = matches!(editor,bareline_app::workspace::WorkspaceEditor::Paged(e) if e.source_changed());
        let Some(path) = w.path(index).filter(|p| self.conflicts.contains(*p) || source_changed) else {
            return Vec::new();
        };
        use bareline_renderer::DrawOp;
        use bareline_ui::{ACCENT, CHROME, TEXT, rect, text};
        let banner = rect(band.x + 8.0, band.y + 4.0, (band.width - 16.0).max(0.0), 62.0);
        ops.push(DrawOp::FillRounded(banner, CHROME, 4.0));
        ops.push(DrawOp::StrokeRounded(banner, ACCENT, 4.0, 1.0));
        // Lead with the file name so a long path never truncates the message;
        // the full path is the tooltip and accessible description (UI-02).
        let message = rect(banner.x + 10.0, banner.y, (banner.width - 20.0).max(0.0), 26.0);
        let label = format!("{} changed on disk · current bytes preserved", banner_name(path));
        ops.push(DrawOp::PushClip(message));
        text(ops, message.x, banner.y + 6.0, label.clone(), 13.0, TEXT);
        ops.push(DrawOp::PopClip);
        self.banners.push((banner, label, path.to_path_buf()));
        let action_width = ((banner.width - 16.0) / 4.0).clamp(0.0, 110.0);
        let mut hits = Vec::new();
        for (i, (label, id)) in [
            ("Compare", "compare.external"),
            ("Reload", "file.external.reload"),
            ("Keep editing", "file.external.keep"),
            ("Save As…", "file.save_as"),
        ]
        .into_iter()
        .enumerate()
        {
            let hit = rect(
                banner.x + 8.0 + i as f32 * action_width,
                banner.y + 30.0,
                action_width,
                28.0,
            );
            ops.push(DrawOp::PushClip(hit));
            text(ops, hit.x + 4.0, hit.y + 5.0, label, 13.0, ACCENT);
            ops.push(DrawOp::PopClip);
            hits.push((hit, bareline_commands::CommandId(id)));
        }
        path_tooltip(message, banner.y + banner.height, path, pointer, ops);
        hits
    }
}
/// Whether document `index` is shown in a view, where its banner is the
/// external-change notification; a background document, or any document while
/// a Settings or Extensions page covers the editor, gets a toast (UI-02). The
/// split pane counts only once its view is loaded, mapped by its loaded tab as
/// the pane's banner is, so a document never loses both notifications.
fn document_shown(
    views: &super::views::ViewsRuntime,
    active: usize,
    page_open: bool,
    w: &bareline_app::workspace::Workspace,
    index: usize,
) -> bool {
    !page_open
        && (index == active
            || views.primary_index(w) == Some(index)
            || (views.open() && views.secondary.is_some() && views.secondary_index(w) == Some(index)))
}

fn is_network_path(path: &std::path::Path) -> bool {
    matches!(path.components().next(),Some(std::path::Component::Prefix(p)) if matches!(p.kind(),std::path::Prefix::UNC(..)|std::path::Prefix::VerbatimUNC(..)))
}
impl Drop for WatchRuntime {
    fn drop(&mut self) {
        for (grant, _) in self.remote_follow.values() {
            grant.revoke();
        }
    }
}
impl Shell {
    fn watch_remote_request(&mut self, el: &ActiveEventLoop, id: &str) {
        if !self.ensure_workspace(el) {
            return;
        }
        use bareline_platform::{RemoteReadAction, RemoteReadGrant};
        let action = match id {
            "file.remote.open" => RemoteReadAction::Open,
            "file.remote.reload" => RemoteReadAction::Reload,
            _ => RemoteReadAction::Follow,
        };
        let path = if action == RemoteReadAction::Open {
            match self.platform.as_ref().map(|p| p.open_file()) {
                Some(Ok(Some(path))) => path,
                _ => return,
            }
        } else {
            let Some(path) = self
                .workspace
                .as_ref()
                .and_then(|w| w.path(self.app.active))
                .map(|p| p.to_owned())
            else {
                return;
            };
            path
        };
        if !WindowsWatchService::confirm_remote_read(&path, action) {
            return;
        }
        let grant = match RemoteReadGrant::after_consent(path.clone(), action, std::time::Duration::from_secs(60)) {
            Ok(grant) => grant,
            Err(error) => {
                self.toasts.push_typed(
                    "watch-remote-grant",
                    toast::next_revision(),
                    bareline_ui::theme::ToastLevel::Error,
                    toast::NotificationKind::Outcome,
                    "Remote file access could not start.",
                    Some(error.to_string()),
                    None,
                    toast::NotificationLifetime::Persistent,
                    Instant::now(),
                );
                return;
            }
        };
        let dirty = self
            .workspace
            .as_ref()
            .and_then(|w| w.editors.get(self.app.active))
            .is_some_and(|e| e.dirty());
        if action == RemoteReadAction::Reload
            && dirty
            && !self
                .platform
                .as_ref()
                .is_some_and(|p| p.confirm_discard_and_reload(&path.display().to_string()))
        {
            grant.revoke();
            return;
        }
        if action == RemoteReadAction::Follow && self.views.pane() == 1 {
            if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = &mut self.views.secondary
                && editor.follow_status().is_none()
            {
                let result = grant
                    .claim(&path, action, std::sync::Arc::new(|| false))
                    .map_err(|e| e.to_string())
                    .and_then(|access| WindowsFileSystem.scoped_remote_read(access).map_err(|e| e.to_string()))
                    .and_then(|fs| {
                        editor.start_follow(fs.clone())?;
                        Ok(fs)
                    });
                match result {
                    Ok(fs) => {
                        if let Some((old, _)) = self.watch.remote_follow.insert(path, (grant, fs)) {
                            old.revoke();
                        }
                    }
                    Err(error) => {
                        grant.revoke();
                        self.toasts.push_typed(
                            "watch-remote-follow",
                            toast::next_revision(),
                            bareline_ui::theme::ToastLevel::Error,
                            toast::NotificationKind::Outcome,
                            "Remote file monitoring could not start.",
                            Some(error),
                            None,
                            toast::NotificationLifetime::Persistent,
                            Instant::now(),
                        );
                    }
                }
                return;
            }
        }
        let Some(w) = &mut self.workspace else {
            grant.revoke();
            return;
        };
        let result = match action {
            RemoteReadAction::Open => w.open_authorized(path, grant),
            RemoteReadAction::Reload => w.reload_authorized(self.app.active, dirty, grant),
            RemoteReadAction::Follow => w.follow_authorized(self.app.active, grant.clone()).map(|fs| {
                if w.path_loading(&path) {
                    self.watch.reopen_pane.insert(path.clone(), self.views.pane());
                    self.watch.reopen_follow.insert(path.clone());
                }
                if let Some((old, _)) = self.watch.remote_follow.insert(path, (grant, fs)) {
                    old.revoke();
                }
            }),
        };
        if let Err(error) = result {
            self.toasts.push_typed(
                "watch-remote-operation",
                toast::next_revision(),
                bareline_ui::theme::ToastLevel::Error,
                toast::NotificationKind::Outcome,
                "Remote file operation failed.",
                Some(error),
                None,
                toast::NotificationLifetime::Persistent,
                Instant::now(),
            );
        }
    }
}
fn should_auto_reload(enabled: bool, dirty: bool, read_only: bool, busy: bool, changed: bool, remote: bool) -> bool {
    enabled && !dirty && !read_only && !busy && changed && !remote
}
fn conflict_notification_id(path: &std::path::Path) -> toast::NotificationId {
    let encoded = bareline_platform::SerializedPath::from_native(path);
    toast::NotificationId(format!("watch-conflict:{}", encoded.data))
}
fn launch_follow_index(workspace: &bareline_app::workspace::Workspace, document: (u64, u64)) -> Option<usize> {
    workspace
        .editors
        .iter()
        .position(|editor| editor.document_identity() == document)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_reload_never_discards_dirty_or_unavailable_source() {
        assert!(should_auto_reload(true, false, false, false, true, false));
        for (dirty, read_only, busy, changed, remote) in [
            (true, false, false, true, false),
            (false, true, false, true, false),
            (false, false, true, true, false),
            (false, false, false, false, false),
            (false, false, false, true, true),
        ] {
            assert!(!should_auto_reload(true, dirty, read_only, busy, changed, remote));
        }
        assert!(!should_auto_reload(false, false, false, false, true, false));
    }
    #[test]
    fn network_path_classification_is_lexical() {
        assert!(is_network_path(std::path::Path::new(r"\\untrusted.example\share\log")));
        assert!(!is_network_path(std::path::Path::new(r"C:\local\log")));
    }
    #[test]
    fn verified_reopen_retires_a_persistent_dirty_period_conflict() {
        fn settle(workspace: &mut bareline_app::workspace::Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
                workspace.pump();
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::yield_now();
            }
        }

        let root = std::env::temp_dir().join(format!(
            "bareline-watch-clean-reopen-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("resident.txt");
        std::fs::write(&path, b"one two\nthree four\nfive six").unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.open(path);
        settle(&mut workspace);
        workspace.editors[0].enqueue(bareline_editor_surface::Input::SetCaret(27, false));
        workspace.editors[0].enqueue(bareline_editor_surface::Input::Insert("YX".into()));
        settle(&mut workspace);

        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.workspace = Some(workspace);
        // The document is in the background, so its conflict is a toast; a
        // shown document gets only its banner (UI-02).
        shell.app.active = 1;
        let canonical = shell.workspace.as_ref().unwrap().path(0).unwrap().to_owned();
        let dirty_identity = shell.workspace.as_ref().unwrap().fingerprint(0).unwrap().identity;
        shell.apply_watch_check_results(vec![(canonical.clone(), dirty_identity, Ok(true))]);
        assert!(shell.watch.conflicts.contains(&canonical));
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.toasts.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            800.0,
            600.0,
            Default::default(),
            &mut Vec::new(),
        );
        let warning = shell.toasts.accessibility().remove(0);
        assert!(warning.text.starts_with("External change detected: resident.txt"));
        let details = warning.details.unwrap();
        assert!(details.contains(&canonical.display().to_string()));
        assert!(details.contains("Choose Keep Current Buffer, Save As, or explicitly reload."));

        let workspace = shell.workspace.as_mut().unwrap();
        assert!(workspace.save(0, canonical.clone()));
        settle(workspace);
        assert!(!workspace.editors[0].dirty());
        assert_eq!(std::fs::read(&canonical).unwrap(), b"one two\nthree four\nfive sixYX");
        let closed_document = workspace.editors[0].snapshot().identity_token();
        workspace
            .close(0, false, &mut bareline_renderer_recording::RecordingBackend::default())
            .unwrap();
        shell.toasts.clear_document(closed_document);
        assert_eq!(shell.toasts.persistent_len(), 1);

        shell.workspace.as_mut().unwrap().open(canonical.clone());
        settle(shell.workspace.as_mut().unwrap());
        let reopened = shell.workspace.as_ref().unwrap();
        assert!(!reopened.editors[0].dirty());
        assert_eq!(std::fs::read(&canonical).unwrap(), b"one two\nthree four\nfive sixYX");
        let reopened_identity = reopened.fingerprint(0).unwrap().identity;
        let opened = WindowsPathTrustProvider
            .open_read(&canonical, PathOrigin::User)
            .unwrap();
        let actual_identity = WindowsFileSystem.identity(&opened.file).unwrap();
        assert_eq!(actual_identity, reopened_identity);
        drop(opened);
        assert_ne!(dirty_identity, reopened_identity);
        shell.watch.requested = false;
        shell.apply_watch_check_results(vec![(canonical.clone(), dirty_identity, Ok(false))]);
        assert!(shell.watch.requested);
        assert!(shell.watch.conflicts.contains(&canonical));
        assert_eq!(shell.toasts.persistent_len(), 1);

        shell.watch.auto_reload_clean = true;
        shell.apply_watch_check_results(vec![(canonical.clone(), reopened_identity, Ok(true))]);
        assert_eq!(
            shell.toasts.persistent_len(),
            1,
            "scheduling reload is not terminal verification"
        );
        assert!(shell.watch.conflicts.contains(&canonical));
        settle(shell.workspace.as_mut().unwrap());
        let verified_identity = shell.workspace.as_ref().unwrap().fingerprint(0).unwrap().identity;
        let opened = WindowsPathTrustProvider
            .open_read(&canonical, PathOrigin::User)
            .unwrap();
        let actual_identity = WindowsFileSystem.identity(&opened.file).unwrap();
        assert_eq!(actual_identity, verified_identity);
        shell.apply_watch_check_results(vec![(
            canonical.clone(),
            verified_identity,
            Ok(actual_identity != verified_identity),
        )]);
        assert!(!shell.watch.conflicts.contains(&canonical));
        assert!(shell.toasts.is_empty());

        drop(opened);
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn launch_follow_resolves_its_workspace_document_when_secondary_is_active() {
        let root = std::env::temp_dir().join(format!(
            "bareline-launch-follow-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let primary = root.join("primary.txt");
        let secondary = root.join("secondary.txt");
        std::fs::write(&primary, "primary\n".repeat(2_000)).unwrap();
        std::fs::write(&secondary, "secondary\n".repeat(2_000)).unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(primary);
        workspace.open(secondary);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let launch_document = workspace.editors[0].document_identity();
        let active_secondary_document = workspace.editors[1].document_identity();
        assert_ne!(launch_document, active_secondary_document);
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.views.test_activate_different_secondary(&mut workspace, 0, 1);
        shell.workspace = Some(workspace);
        shell.watch_start_follow_document(launch_document).unwrap();
        let workspace = shell.workspace.as_ref().unwrap();
        assert!(matches!(
            &workspace.editors[0],
            bareline_app::workspace::WorkspaceEditor::Paged(editor) if editor.follow_status().is_some()
        ));
        assert!(matches!(
            &shell.views.secondary,
            Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) if editor.follow_status().is_none()
        ));
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// UI-02: an external change to a shown document is one banner, in a band
    /// the view reserves under its tab strip (the text moves down; neither
    /// tabs nor text are covered), led by the file name with the full path as
    /// tooltip and accessible description. No duplicate toast.
    #[test]
    fn shown_external_change_is_one_banner_in_a_reserved_band() {
        use bareline_renderer::DrawOp;
        let root = std::env::temp_dir().join(format!(
            "bareline-watch-banner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = root.join("a-deeply-nested-project-directory-with-a-long-name");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("notes.txt");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.open(path);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.workspace = Some(workspace);
        shell.app.active = 0;
        let canonical = shell.workspace.as_ref().unwrap().path(0).unwrap().to_owned();
        let identity = shell.workspace.as_ref().unwrap().fingerprint(0).unwrap().identity;
        shell.apply_watch_check_results(vec![(canonical.clone(), identity, Ok(true))]);
        assert!(shell.watch.conflicts.contains(&canonical));
        assert!(
            shell.toasts.is_empty(),
            "the shown document's banner is its only notification"
        );

        // The band is published before layout and the text moves down by it.
        let workspace = shell.workspace.as_mut().unwrap();
        let bands = shell.watch.banner_bands(workspace);
        workspace.banner_bands = bands;
        assert_eq!(workspace.banner_band(0), CONFLICT_BANNER_BAND);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.draw(0, &mut renderer, 900.0, 600.0, &mut Vec::new()).unwrap();
        let top_inset = workspace.editors[0].viewport().top_inset;
        assert_eq!(top_inset, CONFLICT_BANNER_BAND);
        let view = bareline_ui::rect(0.0, 0.0, 900.0, 576.0);
        let band = banner_band_rect(workspace, 0, view, top_inset, workspace.banner_band(0));
        assert_eq!(band.y, bareline_ui::TAB_HEIGHT, "the band starts under the tab strip");
        assert_eq!(
            band.y + band.height,
            bareline_ui::TAB_HEIGHT + top_inset,
            "and ends at the text"
        );

        let workspace = shell.workspace.as_ref().unwrap();
        let pointer = Point {
            x: band.x + 30.0,
            y: band.y + 12.0,
        };
        let mut operations = Vec::new();
        let hits = shell
            .watch
            .draw_banner(workspace, 0, &workspace.editors[0], band, pointer, &mut operations);
        assert_eq!(hits.len(), 4);
        for op in &operations {
            if let DrawOp::FillRounded(bounds, ..) = op {
                assert!(bounds.y >= band.y && bounds.y + bounds.height <= band.y + band.height);
            }
        }
        for (hit, _) in &hits {
            assert!(hit.y >= band.y && hit.y + hit.height <= band.y + band.height);
        }
        let full = canonical.display().to_string();
        assert!(operations.iter().any(|op| matches!(
            op,
            DrawOp::Text { text, .. } if text.starts_with("notes.txt changed on disk")
        )));
        assert!(
            operations
                .iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if *text == full)),
            "hovering the message shows the full path"
        );
        let nodes = shell.watch_accessibility_nodes();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].name.starts_with("notes.txt changed on disk"));
        assert_eq!(nodes[0].value.as_deref(), Some(full.as_str()));

        // In the background the same event is a toast, retired once the
        // document is shown and its banner takes over.
        shell.app.active = 1;
        shell.apply_watch_check_results(vec![(canonical.clone(), identity, Ok(true))]);
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.app.active = 0;
        shell.watch.retire_shown_conflict_notices(
            shell.workspace.as_ref().unwrap(),
            &shell.views,
            shell.app.active,
            false,
            &mut shell.toasts,
        );
        assert!(shell.toasts.is_empty());

        // While Settings covers the editor the banner is hidden, so the event
        // is a toast until the page is left and the banner shows again.
        shell.settings.controller.show();
        shell.apply_watch_check_results(vec![(canonical.clone(), identity, Ok(true))]);
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.watch.retire_shown_conflict_notices(
            shell.workspace.as_ref().unwrap(),
            &shell.views,
            shell.app.active,
            true,
            &mut shell.toasts,
        );
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.settings.controller.dismiss();
        shell.watch.retire_shown_conflict_notices(
            shell.workspace.as_ref().unwrap(),
            &shell.views,
            shell.app.active,
            false,
            &mut shell.toasts,
        );
        assert!(shell.toasts.is_empty());

        // Bands are keyed by document id, so an edit (or a split pane's sync)
        // that bumps the revision after they are published keeps the band.
        let workspace = shell.workspace.as_mut().unwrap();
        let bands = shell.watch.banner_bands(workspace);
        workspace.banner_bands = bands;
        let before = workspace.editors[0].document_identity();
        workspace.editors[0].enqueue(bareline_editor_surface::Input::Insert("x".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let after = workspace.editors[0].document_identity();
        assert_eq!(after.0, before.0);
        assert_ne!(after.1, before.1, "the edit bumps the revision");
        assert_eq!(workspace.banner_band(0), CONFLICT_BANNER_BAND);
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }
    /// UI-02: a split pane maps its view to a document by its loaded tab, as a
    /// paged clone is a view with a document identity of its own. Its band is
    /// its document's banner, and its document counts as shown, so the change
    /// is never left with neither a banner nor a toast.
    #[test]
    fn split_pane_resolves_its_document_through_its_loaded_tab() {
        let root = std::env::temp_dir().join(format!(
            "bareline-watch-split-banner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let primary = root.join("primary.txt");
        let secondary = root.join("secondary.txt");
        std::fs::write(&primary, "primary\n".repeat(2_000)).unwrap();
        std::fs::write(&secondary, "secondary\n".repeat(2_000)).unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(primary);
        workspace.open(secondary);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.views.test_activate_different_secondary(&mut workspace, 0, 1);
        shell.workspace = Some(workspace);
        let workspace = shell.workspace.as_ref().unwrap();
        assert!(shell.views.open());
        assert!(matches!(
            &shell.views.secondary,
            Some(bareline_app::workspace::WorkspaceEditor::Paged(_))
        ));
        assert_eq!(shell.views.secondary_index(workspace), Some(1));
        assert_eq!(shell.watch.view_banner_band(workspace, &shell.views), 0.0);

        // Document 1 is shown only in the split pane: document 0 is active.
        let changed = workspace.path(1).unwrap().to_owned();
        shell.watch.conflicts.insert(changed);
        assert_eq!(
            shell.watch.view_banner_band(workspace, &shell.views),
            CONFLICT_BANNER_BAND
        );
        assert!(document_shown(&shell.views, 0, false, workspace, 1));
        assert!(
            !document_shown(&shell.views, 0, true, workspace, 1),
            "a page over the editor hides the pane's banner"
        );
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }
}
impl Shell {
    pub(super) fn watch_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.palette.open {
            return false;
        }
        if matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            }
        ) {
            if let Some((_, pane, index, id)) = self
                .watch
                .hits
                .iter()
                .find(|(bounds, _, _, _)| bounds.contains(self.pointer))
                .copied()
            {
                if let Some(w) = &mut self.workspace {
                    if !self.views.activate_watch_pane(w, &mut self.app, pane) {
                        return true;
                    }
                }
                self.app.active = index;
                if let Ok(action) = self.app.commands.dispatch_in(id, &self.command_context()) {
                    self.dispatch(el, action);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
        }
        let away = match event {
            WindowEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta(_, y),
                ..
            } => *y > 0.0,
            WindowEvent::MouseWheel {
                delta: MouseScrollDelta::PixelDelta(p),
                ..
            } => p.y > 0.0,
            WindowEvent::KeyboardInput { event, .. } => {
                event.state == ElementState::Pressed
                    && matches!(
                        event.logical_key,
                        Key::Named(NamedKey::PageUp | NamedKey::Home | NamedKey::ArrowUp)
                    )
            }
            _ => false,
        };
        if away && !self.modifiers.control_key() {
            if let Some(w) = &mut self.workspace {
                if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) =
                    self.views.active_workspace_editor_mut(w, self.app.active)
                {
                    if editor.follow_status().is_some() {
                        editor.set_follow_paused(true);
                    }
                }
            }
        }
        false
    }
}
