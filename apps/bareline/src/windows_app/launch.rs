// SPDX-License-Identifier: MPL-2.0
//! Startup-only, bounded configuration and lossless product CLI parsing.
use bareline_diagnostics::{StartupAction, StartupLedger};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub(super) struct LaunchRuntime {
    requests: Vec<PendingPath>,
    next_request_id: u64,
    /// Text piped to `-`, opened as a new Untitled document with the launch files.
    pub(super) stdin: Option<StdinText>,
    /// `--diag handles` was requested, so handle counters are sampled at startup
    /// and after every document close.
    pub(super) diag_handles: bool,
    drops: DropQueue,
}
/// Paths dropped on the window wait here until their burst has ended (APP-05).
#[derive(Default)]
struct DropQueue {
    /// Paths dropped since the last flush; winit reports each file of one drop as
    /// its own event.
    dropped: Vec<PathBuf>,
    /// A flushed drop being sorted into files and folders off the UI thread.
    sorting: Option<std::sync::mpsc::Receiver<DropBatch>>,
}
impl DropQueue {
    fn push(&mut self, path: PathBuf) {
        self.dropped.push(path);
    }
    /// The running sort's batch once it is ready; a sorter that died yields a notice.
    fn sorted(&mut self) -> Option<DropBatch> {
        let batch = match self.sorting.as_ref()?.try_recv() {
            Ok(batch) => batch,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => DropBatch {
                rejected: vec!["The dropped items could not be inspected.".into()],
                ..Default::default()
            },
        };
        self.sorting = None;
        Some(batch)
    }
    /// Everything dropped since the last flush, as one batch. A drop that arrives
    /// during a sort waits for it, so batches are applied in drop order.
    fn flush(&mut self) -> Option<Vec<PathBuf>> {
        (self.sorting.is_none() && !self.dropped.is_empty()).then(|| std::mem::take(&mut self.dropped))
    }
}
/// Where one sorted drop is sent.
#[derive(Debug, PartialEq, Eq)]
enum DropOpen {
    /// One launch request, so an already open or loading file is activated instead
    /// of opened twice.
    Files(Vec<PathBuf>),
    /// Opened as the workspace folder.
    Folder(PathBuf),
}
/// Sends a sorted drop to `open`, which reports whether it was accepted, and
/// returns everything that was not opened. Only the first folder opens.
fn route_drop(batch: DropBatch, mut open: impl FnMut(DropOpen) -> bool) -> Vec<String> {
    let mut rejected = batch.rejected;
    let count = batch.files.len();
    if count > 0 && !open(DropOpen::Files(batch.files)) {
        rejected.push(format!(
            "{count} dropped files: 256 launch operations are still outstanding"
        ));
    }
    let mut folders = batch.folders.into_iter();
    if let Some(folder) = folders.next() {
        let name = folder.display().to_string();
        if !open(DropOpen::Folder(folder)) {
            rejected.push(format!("{name}: another workspace folder is still opening"));
        }
    }
    rejected
        .extend(folders.map(|folder| format!("{}: only one dropped folder opens as the workspace", folder.display())));
    rejected
}
/// One drop, deduplicated and in drop order.
#[derive(Debug, Default, PartialEq, Eq)]
struct DropBatch {
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    rejected: Vec<String>,
}
/// A path that cannot be inspected counts as a file: its open reports the failure.
fn classify_drop(paths: Vec<PathBuf>, is_folder: impl Fn(&Path) -> bool) -> DropBatch {
    let mut batch = DropBatch::default();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        if !valid_launch_path(&path) {
            batch
                .rejected
                .push(format!("{}: not a valid file path", path.display()));
        } else if is_folder(&path) {
            batch.folders.push(path);
        } else {
            batch.files.push(path);
        }
    }
    batch
}
struct PendingPath {
    id: u64,
    path: PathBuf,
    line: Option<u64>,
    column: u64,
    read_only: bool,
    monitor: bool,
    state: LaunchRequestState,
}
enum LaunchRequestState {
    Queued,
    Opening,
    Opened {
        document: (u64, u64),
        activated: bool,
    },
    Navigating {
        document: (u64, u64),
        source: (u64, u64),
        task: bareline_app::task::Task<Result<usize, String>>,
    },
    Monitoring {
        document: (u64, u64),
        retries: u8,
    },
    Complete,
    Failed,
    Cancelled,
}
impl LaunchRequestState {
    fn terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Cancelled)
    }
}
impl LaunchRuntime {
    pub(super) fn new(config: &LaunchConfig) -> Self {
        let mut runtime = Self {
            requests: Vec::new(),
            next_request_id: 1,
            stdin: None,
            diag_handles: config.diag_handles,
            drops: DropQueue::default(),
        };
        let _ = runtime.queue(&bareline_platform_windows::instance::OpenRequest {
            paths: config.paths.clone(),
            line: config.line,
            column: config.column,
            read_only: config.read_only,
            monitor: config.monitor,
        });
        runtime
    }
    pub(super) fn queue(&mut self, request: &bareline_platform_windows::instance::OpenRequest) -> Option<Vec<u64>> {
        if self.requests.len().saturating_add(request.paths.len()) > 256 {
            return None;
        }
        let mut accepted = Vec::with_capacity(request.paths.len());
        for path in request.paths.iter().cloned() {
            let id = self.next_request_id;
            self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
            accepted.push(id);
            self.requests.push(PendingPath {
                id,
                path,
                line: request.line,
                column: request.column.unwrap_or(1),
                read_only: request.read_only,
                monitor: request.monitor,
                state: LaunchRequestState::Queued,
            });
        }
        Some(accepted)
    }

    pub(super) fn cancel_document(&mut self, document: (u64, u64)) {
        for request in &mut self.requests {
            match &request.state {
                LaunchRequestState::Navigating {
                    document: owner, task, ..
                } if owner.0 == document.0 => {
                    task.cancel();
                    request.state = LaunchRequestState::Cancelled;
                }
                LaunchRequestState::Opened { document: owner, .. }
                | LaunchRequestState::Monitoring { document: owner, .. }
                    if owner.0 == document.0 =>
                {
                    request.state = LaunchRequestState::Cancelled
                }
                _ => {}
            }
        }
        self.requests.retain(|request| !request.state.terminal());
    }

    fn consume_open_outcomes(&mut self, outcomes: Vec<bareline_app::workspace::LaunchOpenOutcome>) -> Option<String> {
        let mut message = None;
        for outcome in outcomes {
            let (request_id, state, failure) = match outcome {
                bareline_app::workspace::LaunchOpenOutcome::Opened { request_id, document } => (
                    request_id,
                    LaunchRequestState::Opened {
                        document,
                        activated: false,
                    },
                    None,
                ),
                bareline_app::workspace::LaunchOpenOutcome::Failed { request_id, error } => {
                    (request_id, LaunchRequestState::Failed, Some(error))
                }
            };
            if let Some(request) = self.requests.iter_mut().find(|request| request.id == request_id) {
                request.state = state;
                // A file that does not exist gets one plain notice (APP-21).
                let failure = failure.map(|error| {
                    if error == bareline_app::workspace::missing_file_message(&request.path) {
                        error
                    } else {
                        format!("Could not open requested file: {error}")
                    }
                });
                message = failure.or(message);
            }
        }
        message
    }

    fn retire_terminal(&mut self) {
        self.requests.retain(|request| !request.state.terminal());
    }

    pub(super) fn has_requests(&self) -> bool {
        !self.requests.is_empty()
    }

    pub(super) fn has_stdin(&self) -> bool {
        self.stdin.is_some()
    }
}
fn request_processing_order(requests: &[PendingPath]) -> Vec<usize> {
    let mut order: Vec<_> = (0..requests.len()).collect();
    order.sort_by_key(|&index| !matches!(requests[index].state, LaunchRequestState::Navigating { .. }));
    order
}
impl super::Shell {
    /// Collects one file of a drop; `launch_drop_pump` flushes the burst as one batch.
    pub(super) fn launch_drop(&mut self, path: PathBuf) {
        self.launch.drops.push(path);
    }

    /// Applies a sorted drop, then hands any newer drop to a sorting thread (APP-05).
    pub(super) fn launch_drop_pump(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        if let Some(batch) = self.launch.drops.sorted() {
            self.launch_drop_apply(el, batch);
        }
        let Some(paths) = self.launch.drops.flush() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let wake = self.wake.clone();
        match std::thread::Builder::new().name("bareline-drop".into()).spawn(move || {
            // A folder check can reach a network share, so it stays off the UI thread.
            let _ = tx.send(classify_drop(paths, |path| {
                std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
            }));
            wake(bareline_app::task::Wake::One(bareline_app::task::Source::Launch));
        }) {
            Ok(_) => self.launch.drops.sorting = Some(rx),
            Err(error) => self.launch_drop_apply(
                el,
                DropBatch {
                    rejected: vec![format!("The dropped items could not be inspected: {error}")],
                    ..Default::default()
                },
            ),
        }
    }

    /// Files join the launch queue, the first folder opens as the workspace, and
    /// everything left over is named in one notice.
    fn launch_drop_apply(&mut self, el: &winit::event_loop::ActiveEventLoop, batch: DropBatch) {
        let rejected = route_drop(batch, |open| match open {
            DropOpen::Files(paths) => {
                // A workspace that cannot start reports its own failure.
                if !self.ensure_workspace(el) {
                    return true;
                }
                let request = bareline_platform_windows::instance::OpenRequest {
                    paths,
                    ..Default::default()
                };
                let accepted = self.launch.queue(&request).is_some();
                if accepted {
                    self.launch_pump();
                }
                accepted
            }
            DropOpen::Folder(folder) => self.panels_open_root(folder),
        });
        if !rejected.is_empty() {
            self.startup_notice(
                "launch:dropped",
                bareline_ui::theme::ToastLevel::Error,
                "Some dropped items were not opened.".into(),
                super::rejected_paths_text(&rejected),
            );
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn launch_pump(&mut self) {
        // Launch files open on top of the restored session, never in place of
        // it or interleaved with it (APP-06). The session pump resumes them.
        if !self.session.restore_settled() {
            return;
        }
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        if let Some(stdin) = self.launch.stdin.take() {
            match workspace.new_document_with_text(stdin.text) {
                Ok(index) => {
                    self.app.active = index;
                    self.session.note_user_focus();
                    self.app.tabs = workspace.titles();
                    if let Some(note) = stdin.note {
                        workspace.message = Some(note);
                    }
                }
                Err(error) => workspace.message = Some(format!("Standard input could not be opened: {error}")),
            }
        }
        let launch_request_ids: Vec<_> = self.launch.requests.iter().map(|request| request.id).collect();
        if let Some(message) = self
            .launch
            .consume_open_outcomes(workspace.take_tracked_open_outcomes(&launch_request_ids))
        {
            workspace.message = Some(message);
        }

        let mut navigation_busy = self
            .launch
            .requests
            .iter()
            .any(|request| matches!(request.state, LaunchRequestState::Navigating { .. }));
        let order = request_processing_order(&self.launch.requests);
        for index in order {
            let request = &mut self.launch.requests[index];
            let state = std::mem::replace(&mut request.state, LaunchRequestState::Cancelled);
            request.state = match state {
                LaunchRequestState::Queued => {
                    if let Some(index) = (0..workspace.editors.len()).find(|&index| {
                        workspace.path(index).or_else(|| workspace.create_target(index)) == Some(request.path.as_path())
                    }) {
                        LaunchRequestState::Opened {
                            document: workspace.editors[index].document_identity(),
                            activated: false,
                        }
                    } else if workspace.path_loading(&request.path) {
                        LaunchRequestState::Queued
                    } else {
                        // A file that does not exist yet opens as a new document
                        // that its first save creates, as Notepad++ offers (APP-09).
                        // Read-only and monitored files are never created; a
                        // missing one gets only the plain not-found notice (APP-21).
                        let opened = if request.read_only || request.monitor {
                            workspace.open_tracked_or_report(request.id, request.path.clone())
                        } else {
                            workspace.open_tracked_or_create(request.id, request.path.clone())
                        };
                        match opened {
                            Ok(()) => LaunchRequestState::Opening,
                            Err(error) => {
                                workspace.message = Some(format!("Could not open requested file: {error}"));
                                LaunchRequestState::Failed
                            }
                        }
                    }
                }
                other => other,
            };

            let state = std::mem::replace(&mut request.state, LaunchRequestState::Cancelled);
            request.state = match state {
                LaunchRequestState::Opened {
                    document,
                    mut activated,
                } => {
                    let Some(index) = workspace
                        .editors
                        .iter()
                        .position(|editor| editor.document_identity() == document)
                    else {
                        workspace.message = Some("Requested document closed before activation.".into());
                        continue;
                    };
                    if !activated {
                        self.app.active = index;
                        // A late restore must not take focus from a requested file (APP-07).
                        self.session.note_user_focus();
                        activated = true;
                    }
                    let editor = &mut workspace.editors[index];
                    if !editor.paged() && !editor.snapshot().is_complete() {
                        LaunchRequestState::Opened { document, activated }
                    } else {
                        if request.read_only {
                            editor.set_read_only(true);
                        }
                        if let Some(line) = request.line {
                            if editor.paged() {
                                if navigation_busy {
                                    LaunchRequestState::Opened { document, activated }
                                } else if let bareline_app::workspace::WorkspaceEditor::Paged(paged) = editor {
                                    let handle = paged.read_handle();
                                    let column = request.column;
                                    let source = handle.snapshot().identity_token();
                                    let wake = self.wake.clone();
                                    match bareline_app::task::spawn(
                                        move || wake(bareline_app::task::Wake::One(bareline_app::task::Source::Launch)),
                                        move |cancel| paged_position(handle, line, column, cancel),
                                    ) {
                                        Ok(task) => {
                                            navigation_busy = true;
                                            LaunchRequestState::Navigating { document, source, task }
                                        }
                                        Err(error) => {
                                            workspace.message = Some(format!("Navigation could not start: {error}"));
                                            LaunchRequestState::Failed
                                        }
                                    }
                                } else {
                                    LaunchRequestState::Failed
                                }
                            } else {
                                match launch_position(editor.snapshot(), line, request.column) {
                                    Ok(offset) => {
                                        editor.viewport_mut().selection.anchor = offset;
                                        editor.viewport_mut().selection.caret = offset;
                                        editor.viewport_mut().scroll_y = (line
                                            .saturating_sub(1)
                                            .min(editor.snapshot().line_count().saturating_sub(1) as u64)
                                            as f64
                                            * 20.0)
                                            .max(0.0);
                                        if request.monitor {
                                            LaunchRequestState::Monitoring { document, retries: 0 }
                                        } else {
                                            LaunchRequestState::Complete
                                        }
                                    }
                                    Err(error) => {
                                        workspace.message = Some(error);
                                        LaunchRequestState::Failed
                                    }
                                }
                            }
                        } else if request.monitor {
                            LaunchRequestState::Monitoring { document, retries: 0 }
                        } else {
                            LaunchRequestState::Complete
                        }
                    }
                }
                LaunchRequestState::Navigating { document, source, task } => match task.poll() {
                    bareline_app::task::TaskPoll::Pending => {
                        if workspace
                            .editors
                            .iter()
                            .any(|editor| paged_source_matches(editor, source))
                        {
                            LaunchRequestState::Navigating { document, source, task }
                        } else {
                            task.cancel();
                            navigation_busy = false;
                            workspace.message = Some("Navigation cancelled because the document closed.".into());
                            LaunchRequestState::Cancelled
                        }
                    }
                    bareline_app::task::TaskPoll::Complete(Ok(offset)) => {
                        navigation_busy = false;
                        let target = workspace
                            .editors
                            .iter_mut()
                            .find(|editor| paged_source_unchanged(editor, source));
                        match target {
                            Some(bareline_app::workspace::WorkspaceEditor::Paged(paged)) => {
                                match paged.restore_selection(
                                    bareline_document::TextOffset(offset),
                                    bareline_document::TextOffset(offset),
                                ) {
                                    Ok(()) if request.monitor => {
                                        LaunchRequestState::Monitoring { document, retries: 0 }
                                    }
                                    Ok(()) => LaunchRequestState::Complete,
                                    Err(error) => {
                                        workspace.message = Some(error);
                                        LaunchRequestState::Failed
                                    }
                                }
                            }
                            _ => {
                                workspace.message =
                                    Some("Navigation target changed or closed; caret unchanged.".into());
                                LaunchRequestState::Cancelled
                            }
                        }
                    }
                    bareline_app::task::TaskPoll::Complete(Err(error)) => {
                        navigation_busy = false;
                        workspace.message = Some(error);
                        LaunchRequestState::Failed
                    }
                    bareline_app::task::TaskPoll::Cancelled => {
                        navigation_busy = false;
                        workspace.message = Some("Navigation cancelled.".into());
                        LaunchRequestState::Cancelled
                    }
                    bareline_app::task::TaskPoll::Failed(error) => {
                        navigation_busy = false;
                        workspace.message = Some(format!("Navigation failed: {error}"));
                        LaunchRequestState::Failed
                    }
                    bareline_app::task::TaskPoll::Consumed => {
                        navigation_busy = false;
                        workspace.message = Some("Navigation result was already consumed.".into());
                        LaunchRequestState::Failed
                    }
                },
                other => other,
            };
        }

        let monitoring: Vec<_> = self
            .launch
            .requests
            .iter()
            .filter_map(|request| match request.state {
                LaunchRequestState::Monitoring { document, retries } => Some((request.id, document, retries)),
                _ => None,
            })
            .collect();
        for (request_id, document, retries) in monitoring {
            let outcome = self.watch_start_follow_document(document);
            if let Some(request) = self.launch.requests.iter_mut().find(|request| request.id == request_id) {
                match outcome {
                    Ok(()) => request.state = LaunchRequestState::Complete,
                    Err(error) if error.contains("queue is full") && retries < 2 => {
                        request.state = LaunchRequestState::Monitoring {
                            document,
                            retries: retries + 1,
                        };
                        if let Some(workspace) = &mut self.workspace {
                            workspace.message = Some(format!("Monitor startup is busy; retrying ({}/3).", retries + 1));
                        }
                    }
                    Err(error) => {
                        request.state = LaunchRequestState::Failed;
                        if let Some(workspace) = &mut self.workspace {
                            workspace.message = Some(format!("Monitor startup failed: {error}"));
                        }
                    }
                }
            }
        }
        self.launch.retire_terminal();
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use std::time::Duration;

    fn runtime() -> LaunchRuntime {
        LaunchRuntime {
            requests: Vec::new(),
            next_request_id: 1,
            stdin: None,
            diag_handles: false,
            drops: DropQueue::default(),
        }
    }

    fn request(paths: Vec<PathBuf>) -> bareline_platform_windows::instance::OpenRequest {
        bareline_platform_windows::instance::OpenRequest {
            paths,
            line: Some(7),
            column: Some(3),
            read_only: true,
            monitor: true,
        }
    }

    #[test]
    fn sequential_failures_release_capacity_and_preserve_request_ids() {
        let mut launch = runtime();
        for number in 0..300 {
            assert!(
                launch
                    .queue(&request(vec![PathBuf::from(format!("missing-{number}"))]))
                    .is_some()
            );
            let id = launch.requests.last().unwrap().id;
            let message = launch.consume_open_outcomes(vec![bareline_app::workspace::LaunchOpenOutcome::Failed {
                request_id: id,
                error: "missing".into(),
            }]);
            assert!(message.unwrap().contains("missing"));
            launch.retire_terminal();
            assert!(launch.requests.is_empty());
        }
        assert_eq!(launch.next_request_id, 301);
    }

    #[test]
    fn a_missing_file_gets_one_plain_notice() {
        let mut launch = runtime();
        let missing = PathBuf::from(r"C:\absent\notes.txt");
        let ids = launch
            .queue(&request(vec![missing.clone(), PathBuf::from("locked.txt")]))
            .unwrap();
        let plain = bareline_app::workspace::missing_file_message(&missing);
        let message = launch.consume_open_outcomes(vec![bareline_app::workspace::LaunchOpenOutcome::Failed {
            request_id: ids[0],
            error: plain.clone(),
        }]);
        // APP-21: no "Could not open" wrapper, recovery wording or retry offer.
        assert_eq!(message, Some(plain));
        let message = launch.consume_open_outcomes(vec![bareline_app::workspace::LaunchOpenOutcome::Failed {
            request_id: ids[1],
            error: "Access is denied.".into(),
        }]);
        assert_eq!(
            message.as_deref(),
            Some("Could not open requested file: Access is denied.")
        );
    }

    #[test]
    fn concurrent_bound_rejects_then_reopens_one_slot_with_flags_intact() {
        let mut launch = runtime();
        let duplicate = PathBuf::from("same.txt");
        assert!(launch.queue(&request(vec![duplicate; 256])).is_some());
        assert!(launch.queue(&request(vec![PathBuf::from("overflow.txt")])).is_none());
        assert_eq!(launch.requests[0].id, 1);
        assert_eq!(launch.requests[255].id, 256);
        assert!(launch.requests[0].read_only);
        assert!(launch.requests[0].monitor);
        assert_eq!(launch.requests[0].line, Some(7));
        assert_eq!(launch.requests[0].column, 3);
        launch.consume_open_outcomes(vec![bareline_app::workspace::LaunchOpenOutcome::Failed {
            request_id: 1,
            error: "missing".into(),
        }]);
        launch.retire_terminal();
        assert!(launch.queue(&request(vec![PathBuf::from("later-valid.txt")])).is_some());
        assert_eq!(launch.requests.last().unwrap().id, 257);
    }

    #[test]
    fn one_drop_becomes_one_deduplicated_batch() {
        let first = PathBuf::from(r"C:\drop\a.txt");
        let second = PathBuf::from(r"C:\drop\b.txt");
        let folder = PathBuf::from(r"C:\drop\project");
        let batch = classify_drop(
            vec![
                first.clone(),
                folder.clone(),
                second.clone(),
                first.clone(),
                folder.clone(),
                PathBuf::from("relative.txt"),
            ],
            |path| path == folder,
        );
        assert_eq!(
            batch,
            DropBatch {
                files: vec![first.clone(), second.clone()],
                folders: vec![folder],
                rejected: vec!["relative.txt: not a valid file path".into()],
            }
        );
        // The files join the launch queue together, in drop order.
        let mut launch = runtime();
        let ids = launch
            .queue(&bareline_platform_windows::instance::OpenRequest {
                paths: batch.files,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(launch.requests[0].path, first);
        assert_eq!(launch.requests[1].path, second);
        assert!(!launch.requests[0].read_only);
        assert_eq!(launch.requests[0].line, None);
    }

    #[test]
    fn drops_are_batched_per_burst_and_applied_in_order() {
        let mut drops = DropQueue::default();
        assert_eq!(drops.flush(), None);
        // winit reports one drop of two files as two events; one flush takes both.
        drops.push(PathBuf::from(r"C:\drop\a.txt"));
        drops.push(PathBuf::from(r"C:\drop\b.txt"));
        let burst = drops.flush().unwrap();
        assert_eq!(
            burst,
            vec![PathBuf::from(r"C:\drop\a.txt"), PathBuf::from(r"C:\drop\b.txt")]
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        drops.sorting = Some(rx);
        // A second drop while the first is being sorted waits for it.
        drops.push(PathBuf::from(r"C:\drop\c.txt"));
        assert_eq!(drops.sorted(), None);
        assert_eq!(drops.flush(), None);
        tx.send(classify_drop(burst, |_| false)).unwrap();
        assert_eq!(
            drops.sorted().unwrap().files,
            vec![PathBuf::from(r"C:\drop\a.txt"), PathBuf::from(r"C:\drop\b.txt")]
        );
        assert_eq!(drops.flush(), Some(vec![PathBuf::from(r"C:\drop\c.txt")]));
        assert_eq!(drops.flush(), None);
        // A sorter that died still ends its sort, with a notice.
        let (tx, rx) = std::sync::mpsc::sync_channel::<DropBatch>(1);
        drops.sorting = Some(rx);
        drop(tx);
        assert_eq!(drops.sorted().unwrap().rejected.len(), 1);
        assert!(drops.sorting.is_none());
    }

    #[test]
    fn dropped_files_open_as_one_request_and_the_first_folder_as_workspace() {
        let batch = || DropBatch {
            files: vec![PathBuf::from(r"C:\drop\a.txt"), PathBuf::from(r"C:\drop\b.txt")],
            folders: vec![PathBuf::from(r"C:\drop\one"), PathBuf::from(r"C:\drop\two")],
            rejected: vec!["relative.txt: not a valid file path".into()],
        };
        let mut opened = Vec::new();
        let rejected = route_drop(batch(), |open| {
            opened.push(open);
            true
        });
        assert_eq!(
            opened,
            vec![
                DropOpen::Files(vec![PathBuf::from(r"C:\drop\a.txt"), PathBuf::from(r"C:\drop\b.txt")]),
                DropOpen::Folder(PathBuf::from(r"C:\drop\one")),
            ]
        );
        assert_eq!(
            rejected,
            vec![
                "relative.txt: not a valid file path".to_string(),
                r"C:\drop\two: only one dropped folder opens as the workspace".to_string(),
            ]
        );
        // Refusals are named in the same notice.
        let rejected = route_drop(batch(), |_| false);
        assert_eq!(
            rejected,
            vec![
                "relative.txt: not a valid file path".to_string(),
                "2 dropped files: 256 launch operations are still outstanding".to_string(),
                r"C:\drop\one: another workspace folder is still opening".to_string(),
                r"C:\drop\two: only one dropped folder opens as the workspace".to_string(),
            ]
        );
        // A drop of files only never touches the workspace folder.
        let mut opened = Vec::new();
        let rejected = route_drop(
            DropBatch {
                files: vec![PathBuf::from(r"C:\drop\a.txt")],
                ..Default::default()
            },
            |open| {
                opened.push(open);
                true
            },
        );
        assert_eq!(opened, vec![DropOpen::Files(vec![PathBuf::from(r"C:\drop\a.txt")])]);
        assert!(rejected.is_empty());
    }

    #[test]
    fn completed_navigation_is_processed_before_an_earlier_waiter() {
        let root = std::env::temp_dir().join(format!(
            "bareline-launch-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let waiting_path = root.join("waiting.txt");
        let completed_path = root.join("completed.txt");
        std::fs::write(&waiting_path, "waiting\n".repeat(2_000)).unwrap();
        std::fs::write(&completed_path, "completed\n".repeat(2_000)).unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(waiting_path.clone());
        workspace.open(completed_path.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let waiting_document = workspace.editors[0].document_identity();
        let completed_document = workspace.editors[1].document_identity();
        let bareline_app::workspace::WorkspaceEditor::Paged(completed) = &workspace.editors[1] else {
            panic!("forced-paged fixture opened resident editor");
        };
        let completed_source = completed.snapshot().identity_token();
        let pool = bareline_app::task::Pool::new(1, 2);
        let (completed_tx, completed_rx) = std::sync::mpsc::sync_channel(1);
        let task = pool
            .spawn(
                move || {
                    completed_tx.send(()).unwrap();
                },
                move |_| Ok::<_, String>(3),
            )
            .unwrap();
        completed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut shell = super::super::accessibility::tests::headless_shell();
        let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(2);
        shell.wake = std::sync::Arc::new(move |wake| {
            wake_tx.send(wake).unwrap();
        });
        shell.workspace = Some(workspace);
        shell.launch.requests.push(PendingPath {
            id: 1,
            path: waiting_path,
            line: Some(8),
            column: 1,
            read_only: false,
            monitor: false,
            state: LaunchRequestState::Opened {
                document: waiting_document,
                activated: true,
            },
        });
        shell.launch.requests.push(PendingPath {
            id: 2,
            path: completed_path,
            line: Some(9),
            column: 1,
            read_only: false,
            monitor: false,
            state: LaunchRequestState::Navigating {
                document: completed_document,
                source: completed_source,
                task,
            },
        });
        shell.launch_pump();
        assert_eq!(shell.launch.requests.len(), 1);
        assert_eq!(shell.launch.requests[0].id, 1);
        assert!(matches!(
            shell.launch.requests[0].state,
            LaunchRequestState::Navigating { .. }
        ));
        assert_eq!(
            wake_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            bareline_app::task::Wake::One(bareline_app::task::Source::Launch)
        );
        shell.launch_pump();
        assert!(shell.launch.requests.is_empty());
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closing_navigation_owner_cancels_task_and_releases_slot() {
        let pool = bareline_app::task::Pool::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let task = pool
            .spawn(
                || {},
                move |cancel| {
                    started_tx.send(()).unwrap();
                    while !cancel.is_cancelled() {
                        std::thread::yield_now();
                    }
                    Ok(0)
                },
            )
            .unwrap();
        let cancellation = task.cancel_handle();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let mut launch = runtime();
        launch.requests.push(PendingPath {
            id: 1,
            path: PathBuf::from("closed.txt"),
            line: Some(10),
            column: 1,
            read_only: false,
            monitor: false,
            state: LaunchRequestState::Navigating {
                document: (11, 22),
                source: (11, 22),
                task,
            },
        });
        launch.cancel_document((11, 23));
        assert!(cancellation.is_cancelled());
        assert!(launch.requests.is_empty());

        let next = pool.spawn(|| {}, |_| Ok::<_, String>(9)).unwrap();
        assert_eq!(
            next.wait_timeout(Duration::from_secs(5)),
            bareline_app::task::TaskPoll::Complete(Ok(9))
        );
    }

    #[test]
    fn forced_paged_navigation_uses_full_source_after_viewport_moves() {
        let root = std::env::temp_dir().join(format!(
            "bareline-launch-paged-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("paged.txt");
        std::fs::write(&path, "line\n".repeat(2_000)).unwrap();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(path);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        workspace.editors[0].scroll(10_000.0, 400.0);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let bareline_app::workspace::WorkspaceEditor::Paged(paged) = &workspace.editors[0] else {
            panic!("forced-paged fixture opened resident editor");
        };
        let handle = paged.read_handle();
        let source = handle.snapshot().identity_token();
        assert_ne!(source, workspace.editors[0].snapshot().identity_token());
        assert!(paged_source_matches(&workspace.editors[0], source));
        assert!(paged_source_unchanged(&workspace.editors[0], source));
        assert_eq!(
            paged_position(handle, 1_500, 3, &bareline_app::task::Cancel::default()).unwrap(),
            7_497
        );
        workspace.editors[0].enqueue(bareline_app::workspace::Input::Insert("x".into()));
        let mutation_deadline = std::time::Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < mutation_deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert!(paged_source_matches(&workspace.editors[0], source));
        assert!(!paged_source_unchanged(&workspace.editors[0], source));
        drop(workspace);
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl Drop for LaunchRuntime {
    fn drop(&mut self) {
        for request in &self.requests {
            if let LaunchRequestState::Navigating { task, .. } = &request.state {
                task.cancel();
            }
        }
    }
}

fn paged_source_matches(editor: &bareline_app::workspace::WorkspaceEditor, source: (u64, u64)) -> bool {
    matches!(
        editor,
        bareline_app::workspace::WorkspaceEditor::Paged(paged)
            if paged.snapshot().identity_token().0 == source.0
    )
}

fn paged_source_unchanged(editor: &bareline_app::workspace::WorkspaceEditor, source: (u64, u64)) -> bool {
    matches!(
        editor,
        bareline_app::workspace::WorkspaceEditor::Paged(paged)
            if paged.snapshot().identity_token() == source
    )
}

fn paged_position(
    handle: bareline_editor_surface::paged_view::PagedReadHandle,
    line: u64,
    column: u64,
    cancel: &bareline_app::task::Cancel,
) -> Result<usize, String> {
    use bareline_document::{
        Budget, TextOffset,
        line_lookup::{LineLookupPoll, LineTarget},
        paged::{SparseLineIndex, WindowPoll},
    };
    let snapshot = handle.snapshot();
    let budget = Budget::new(256 * 1024);
    let index = SparseLineIndex::new(snapshot.clone(), 16, 65536, &budget).map_err(|e| format!("Line index: {e:?}"))?;
    let mut lookup = index
        .lookup(
            LineTarget::Line(usize::try_from(line.saturating_sub(1)).map_err(|_| "Line number too large")?),
            budget.clone(),
        )
        .map_err(|e| format!("Line lookup: {e:?}"))?;
    let range = loop {
        if cancel.is_cancelled() {
            return Err("Navigation cancelled".into());
        }
        match lookup.poll() {
            LineLookupPoll::Range(range) => break range,
            LineLookupPoll::Pending(ticket) => {
                if !handle.resolve_page(ticket).map_err(|error| error.to_string())? {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            LineLookupPoll::Progress(_) => (),
            LineLookupPoll::Failed(bareline_document::Error::OutOfBounds) => return Ok(snapshot.len()),
            other => return Err(format!("Line lookup: {other:?}")),
        }
    };
    let mut offset = range.start.0;
    let mut columns = column.saturating_sub(1);
    while offset < range.end.0 && columns > 0 {
        if cancel.is_cancelled() {
            return Err("Navigation cancelled".into());
        }
        let mut request = snapshot
            .begin_viewport(TextOffset(offset), 65536, &budget)
            .map_err(|e| format!("Column window: {e:?}"))?;
        let window = loop {
            if cancel.is_cancelled() {
                return Err("Navigation cancelled".into());
            }
            match request.poll() {
                WindowPoll::Ready(w) => break w,
                WindowPoll::Pending(ticket) => {
                    if !handle.resolve_page(ticket).map_err(|error| error.to_string())? {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                _ => return Err("Column lookup unavailable".into()),
            }
        };
        let start = offset.saturating_sub(window.range().start.0);
        let text = &window.text()[start..];
        let mut moved = 0;
        for ch in text.chars() {
            if columns == 0 || ch == '\r' || ch == '\n' {
                return Ok(offset + moved);
            }
            moved += ch.len_utf8();
            columns -= 1;
        }
        if moved == 0 {
            break;
        }
        offset += moved;
    }
    Ok(offset)
}

fn launch_position(snapshot: &bareline_document::DocumentSnapshot, line: u64, column: u64) -> Result<usize, String> {
    let line = usize::try_from(line.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(snapshot.line_count().saturating_sub(1));
    let range = snapshot
        .line_range(line)
        .map_err(|error| format!("Cannot navigate to the requested line: {error:?}"))?;
    if range.end.0 - range.start.0 > 64 * 1024 {
        return Err("The requested line exceeds the bounded command-line navigation window.".into());
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LaunchMode {
    Help,
    Version,
    Diagnostic,
    Portable,
    Installed,
    Performance,
}

// Internal diagnostic switches (--smoke, --perf, --diag, --diagnostic-root) are
// deliberately not advertised.
pub(super) const HELP: &str = "Usage: bareline [OPTIONS] [--] [FILE ...]

Opens up to 16 files on top of the restored session; further files are listed
as not opened. A file that does not exist opens as a new document and is
created when you save it. Use -- before file names that begin with '-'.

Options:
  -                 Read standard input into a new Untitled document, for at
                    most 10 seconds. With no files, or when a running window
                    takes the files, the text opens in a separate window that
                    neither restores nor saves the session
  --line N          Go to line N (one-based) in the opened files
  --column N        Go to column N on that line (requires --line; the
                    Notepad++ -c<column> alone uses line 1)
  --read-only       Open the files read-only
  --monitor         Open read-only and follow changes to the files
  --no-session      Do not restore or save the previous session
  --no-extensions   Start without extensions
  --new-instance    Open a separate window instead of reusing a running one
  --software        Use software rendering (the default)
  --hardware        Use hardware (GPU) rendering
  -h, --help        Show this help
  -V, --version     Show the version

Notepad++ spellings are accepted: -n<line> -c<column> -ro -multiInst
-nosession -noPlugin; -notabbar is ignored.";

/// The instance handoff carries at most this many files per launch (APP-09).
const MAX_LAUNCH_PATHS: usize = 16;
/// Piped text beyond this is left unread and reported (APP-09).
const MAX_STDIN_BYTES: usize = 64 << 20;

/// Standard input read for `-`, with a notice when it was cut short or was not
/// valid text in a recognized encoding.
pub(super) struct StdinText {
    pub(super) text: String,
    pub(super) note: Option<String>,
}

pub(super) struct ParsedLaunch {
    mode: LaunchMode,
    performance: Option<super::performance::PerformanceOptions>,
    options: bareline_distribution::cli::LaunchOptions,
    software: bool,
    hardware: bool,
    smoke: bool,
    prototype: bool,
    perf: bool,
    diag_handles: bool,
    diagnostic_root: Option<PathBuf>,
}
impl ParsedLaunch {
    pub(super) fn mode(&self) -> LaunchMode {
        self.mode
    }
    pub(super) fn has_paths(&self) -> bool {
        !self.options.paths.is_empty() || self.options.stdin
    }
}

#[derive(Clone)]
pub(super) struct ProfileInitialization {
    roaming: Option<PathBuf>,
    local: Option<PathBuf>,
    temp: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) struct ProfileInitializationResult {
    pub(super) profile_root: Option<PathBuf>,
    pub(super) migration: Result<bareline_file_io::profile_migration::MigrationReport, String>,
    pub(super) authorities: bareline_file_io::profile_migration::MigrationReport,
    pub(super) cleanup: bareline_file_io::owned_cache::SweepReport,
    /// The user settings this migration published locally, read and parsed on
    /// the worker so the UI thread only reconciles them (APP-12). `None` when
    /// migration did not publish a local settings file.
    pub(super) migrated_settings: Option<Result<bareline_settings::SettingsDocument, String>>,
}

#[derive(Default)]
pub(super) struct ProfileInitializationRuntime {
    pending: Option<ProfileInitialization>,
    retry: Option<ProfileInitialization>,
    worker: Option<std::sync::mpsc::Receiver<Result<ProfileInitializationResult, String>>>,
    completion: Option<Result<ProfileInitializationResult, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ProfileInitializationRuntime {
    pub(super) fn new(pending: Option<ProfileInitialization>) -> Self {
        Self {
            retry: pending.clone(),
            pending,
            worker: None,
            completion: None,
            cancel: Default::default(),
        }
    }

    pub(super) fn retry(&mut self) -> Result<(), String> {
        if self.worker.is_some() {
            return Err("Profile migration is already running".into());
        }
        self.pending = self.retry.clone();
        self.completion = None;
        self.pending
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| "Profile migration is not available in this launch mode".into())
    }

    pub(super) fn schedule(&mut self, notify: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<bool, String> {
        let Some(initialization) = self.pending.take() else {
            return Ok(false);
        };
        let retry = initialization.clone();
        self.cancel.store(false, std::sync::atomic::Ordering::Release);
        let cancel = self.cancel.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-profile-initialize".into())
            .spawn(move || {
                let profile_root = initialization.local.clone();
                let migration = match (initialization.roaming.as_deref(), initialization.local.as_deref()) {
                    (Some(roaming), Some(local)) => {
                        let retire_sources = bareline_file_io::profile_migration::retirement_ready(
                            local,
                            &bareline_platform_windows::WindowsFileSystem,
                        );
                        bareline_file_io::profile_migration::migrate(
                            bareline_file_io::profile_migration::MigrationRequest {
                                roaming,
                                local,
                                retire_sources,
                                max_entries: 16_384,
                                max_io_bytes: 8 * 1024 * 1024 * 1024,
                                max_time: std::time::Duration::from_secs(30),
                            },
                            &bareline_platform_windows::WindowsFileSystem,
                            &|| cancel.load(std::sync::atomic::Ordering::Acquire),
                        )
                        .map_err(|error| error.to_string())
                    }
                    _ => Ok(Default::default()),
                };
                let migrated_settings = match (&migration, initialization.local.as_deref()) {
                    (Ok(report), Some(local))
                        if report.items.iter().any(|item| {
                            item.name == "settings.toml"
                                && item.migrated
                                && item.destination_present
                                && item.authority == bareline_file_io::profile_migration::ReadAuthority::Local
                        }) =>
                    {
                        Some(super::settings::read_migrated_user(&local.join("settings.toml")))
                    }
                    _ => None,
                };
                let authorities = match (initialization.roaming.as_deref(), initialization.local.as_deref()) {
                    (Some(roaming), Some(local)) => bareline_file_io::profile_migration::inspect_authorities(
                        roaming,
                        local,
                        &bareline_platform_windows::WindowsFileSystem,
                    ),
                    _ => Default::default(),
                };
                let cleanup = bareline_file_io::owned_cache::sweep(
                    &initialization.temp,
                    &std::collections::HashSet::new(),
                    &bareline_platform_windows::WindowsFileSystem,
                    &|| cancel.load(std::sync::atomic::Ordering::Acquire),
                    256,
                    std::time::Duration::from_millis(100),
                );
                let _ = sender.send(Ok(ProfileInitializationResult {
                    profile_root,
                    migration,
                    authorities,
                    cleanup,
                    migrated_settings,
                }));
                notify();
            }) {
            Ok(_) => {
                self.worker = Some(receiver);
                self.completion = None;
                Ok(true)
            }
            Err(error) => {
                let error = format!("Cannot schedule profile initialization: {error}");
                self.pending = Some(retry);
                self.completion = Some(Err(error.clone()));
                Err(error)
            }
        }
    }

    pub(super) fn pump(&mut self) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        let completion = match worker.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Profile initialization worker stopped without a result".into())
            }
        };
        self.worker = None;
        self.completion = Some(completion);
        true
    }

    /// Retained completion receipt for PR-T02 to reconcile migrated settings and
    /// session state against revisions created after the first frame.
    #[cfg(test)]
    pub(super) fn completion(&self) -> Option<&Result<ProfileInitializationResult, String>> {
        self.completion.as_ref()
    }

    pub(super) fn take_completion(&mut self) -> Option<Result<ProfileInitializationResult, String>> {
        self.completion.take()
    }

    pub(super) fn settled(&self) -> bool {
        self.worker.is_none() && (self.pending.is_none() || self.completion.is_some())
    }
}
impl Drop for ProfileInitializationRuntime {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Release);
    }
}

pub struct LaunchConfig {
    pub(super) mode: LaunchMode,
    pub performance: Option<super::performance::PerformanceConfig>,
    pub(super) profile_initialization: Option<ProfileInitialization>,
    pub portable: bool,
    pub settings_path: Option<PathBuf>,
    pub(super) legacy_settings_path: Option<PathBuf>,
    pub session_path: Option<PathBuf>,
    pub(super) legacy_session_path: Option<PathBuf>,
    pub recovery_path: Option<PathBuf>,
    pub(super) legacy_recovery_path: Option<PathBuf>,
    pub extensions_path: Option<PathBuf>,
    pub(super) legacy_extensions_path: Option<PathBuf>,
    pub diagnostics_path: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
    /// `path: reason` for each argument that could not become a file path.
    pub(super) rejected_paths: Vec<String>,
    /// Standard input, read before the instance handoff because it cannot be forwarded.
    pub(super) stdin: Option<StdinText>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub read_only: bool,
    pub monitor: bool,
    pub no_session: bool,
    pub no_extensions: bool,
    pub new_instance: bool,
    pub software: bool,
    pub hardware: bool,
    pub smoke: bool,
    pub prototype: bool,
    pub perf: bool,
    pub diag_handles: bool,
}

pub(super) fn parse(args: &[OsString], ledger: &mut StartupLedger) -> Result<ParsedLaunch, Box<dyn std::error::Error>> {
    ledger.record(StartupAction::ParseCli);
    let (filtered, performance) = super::performance::parse_args(args)?;
    let args = &filtered;
    let mut product = Vec::new();
    let (mut software, mut hardware, mut smoke, mut prototype, mut perf) = (false, false, false, false, false);
    let mut after_separator = false;
    let mut diag: Option<OsString> = None;
    let mut diagnostic_root: Option<PathBuf> = None;
    let mut args_iter = args.iter();
    while let Some(arg) = args_iter.next() {
        if !after_separator && arg == "--diag" {
            diag = Some(args_iter.next().ok_or("Missing diagnostic option value")?.clone());
            continue;
        }
        if !after_separator && arg == "--diag=handles" {
            diag = Some(OsString::from("handles"));
            continue;
        }
        if !after_separator && arg == "--diagnostic-root" {
            let value = args_iter.next().ok_or("Missing diagnostic root")?;
            if diagnostic_root.replace(PathBuf::from(value)).is_some() {
                return Err("Duplicate diagnostic root".into());
            }
            continue;
        }
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
    if options.help && options.version {
        return Err("Choose either --help or --version".into());
    }
    if software && hardware {
        return Err("Choose either --software or --hardware".into());
    }
    if diag.as_deref().is_some_and(|value| value != "handles") {
        return Err("Unknown diagnostic option".into());
    }
    let documents = !options.paths.is_empty() || options.stdin;
    if performance.is_some() && documents {
        return Err("Performance workloads reject ordinary document paths".into());
    }
    // More than 16 paths is not an error: `prepare` opens the first 16 and names the rest.
    if documents && (smoke || perf || prototype) {
        return Err("Diagnostic modes do not accept document paths.".into());
    }
    let diagnostic_count = usize::from(smoke) + usize::from(perf) + usize::from(prototype);
    if diagnostic_count > 1 {
        return Err("Choose one diagnostic mode".into());
    }
    if performance.is_some() && (diagnostic_count != 0 || diag.is_some()) {
        return Err("Performance workload cannot be combined with diagnostic modes".into());
    }
    let mode = if options.help {
        LaunchMode::Help
    } else if options.version {
        LaunchMode::Version
    } else if performance.is_some() {
        LaunchMode::Performance
    } else if diagnostic_count != 0 || diag.is_some() {
        LaunchMode::Diagnostic
    } else {
        LaunchMode::Installed
    };
    if diagnostic_root.is_some() && mode != LaunchMode::Diagnostic {
        return Err("--diagnostic-root requires a diagnostic mode".into());
    }
    Ok(ParsedLaunch {
        mode,
        performance,
        options,
        software,
        hardware,
        smoke,
        prototype,
        perf,
        diag_handles: diag.is_some(),
        diagnostic_root,
    })
}

pub(super) fn prepare(
    parsed: ParsedLaunch,
    ledger: &mut StartupLedger,
) -> Result<LaunchConfig, Box<dyn std::error::Error>> {
    debug_assert!(!matches!(parsed.mode, LaunchMode::Help | LaunchMode::Version));
    let executable = std::env::current_exe()?;
    let directory = executable.parent().ok_or("executable directory unavailable")?;
    // Capture this before process search hardening changes the process CWD.
    let cwd = std::env::current_dir()?;
    let performance = parsed.performance.map(super::performance::prepare).transpose()?;
    // The portable marker is configuration discovery, so informational and invalid
    // invocations return before it is read.
    let portable = matches!(parsed.mode, LaunchMode::Installed | LaunchMode::Diagnostic)
        && parsed.diagnostic_root.is_none()
        && portable_marker(directory, ledger);
    let mode = select_mode(parsed.mode, portable);
    let diagnostic_root = parsed
        .diagnostic_root
        .map(|root| validate_diagnostic_root(&root))
        .transpose()?;
    // Settings, session, recovery journals and macros are machine-local data.
    // Installed locations are not even discovered for an isolated launch.
    let (roaming, local) = if mode == LaunchMode::Installed {
        (
            std::env::var_os("APPDATA").map(|root| PathBuf::from(root).join("Bareline")),
            std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Bareline")),
        )
    } else {
        (None, None)
    };
    let installed = local.clone().or_else(|| roaming.clone());
    let root = match mode {
        LaunchMode::Performance => performance.as_ref().map(|config| config.root.clone()),
        LaunchMode::Portable => bareline_distribution::data_root(&executable, true, directory),
        LaunchMode::Diagnostic => {
            Some(diagnostic_root.ok_or("Diagnostic mode requires --diagnostic-root or a portable executable")?)
        }
        LaunchMode::Installed => installed,
        LaunchMode::Help | LaunchMode::Version => unreachable!(),
    };
    let legacy = legacy_root(mode, roaming.clone());
    let profile_initialization = profile_initialization(mode, roaming.clone(), local.clone(), std::env::temp_dir());
    // An unusable argument is reported with its file; it never stops the launch (APP-17).
    let (paths, rejected_paths) = launch_paths(&cwd, parsed.options.paths);
    let stdin = parsed.options.stdin.then(read_stdin);
    let config = LaunchConfig {
        mode,
        profile_initialization,
        portable,
        settings_path: root.as_ref().map(|p| p.join("settings.toml")),
        legacy_settings_path: legacy.as_ref().map(|p| p.join("settings.toml")),
        session_path: root.as_ref().map(|p| p.join("session.json")),
        legacy_session_path: legacy.as_ref().map(|p| p.join("session.json")),
        recovery_path: root.as_ref().map(|p| p.join("recovery")),
        legacy_recovery_path: legacy.as_ref().map(|p| p.join("recovery")),
        extensions_path: root.as_ref().map(|p| p.join("extensions")),
        legacy_extensions_path: legacy.as_ref().map(|p| p.join("extensions")),
        diagnostics_path: root.as_ref().map(|path| path.join("diagnostics")),
        paths,
        rejected_paths,
        stdin,
        line: parsed.options.line,
        column: parsed.options.column,
        read_only: parsed.options.read_only,
        monitor: parsed.options.monitor,
        no_session: parsed.options.no_session || performance.is_some(),
        no_extensions: parsed.options.no_extensions
            || performance.as_ref().is_some_and(|config| !config.requires_extensions()),
        new_instance: parsed.options.new_instance || performance.is_some(),
        software: parsed.software,
        hardware: parsed.hardware,
        smoke: parsed.smoke,
        prototype: parsed.prototype,
        perf: parsed.perf,
        diag_handles: parsed.diag_handles,
        performance,
    };
    // Relative command line paths were resolved against the launch directory above;
    // from here the process must not search it for executables or DLLs (SEC-01).
    let _ = bareline_platform_windows::shell_integration::harden_process_search_paths();
    if config.diag_handles {
        log_handle_counters(config.diagnostics_path.as_deref());
    }
    Ok(config)
}

/// Resolves the command-line files. The first 16 usable ones open; every other
/// argument is named with the reason it was not opened (APP-09, APP-17).
fn launch_paths(cwd: &Path, arguments: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
    let (mut paths, mut rejected) = (Vec::new(), Vec::new());
    for path in arguments {
        match resolve_launch_path(cwd, &path) {
            Ok(resolved) if paths.len() < MAX_LAUNCH_PATHS => paths.push(resolved),
            Ok(_) => rejected.push(format!(
                "{}: only the first {MAX_LAUNCH_PATHS} files of a launch are opened",
                path.display()
            )),
            Err(reason) => rejected.push(format!("{}: {reason}", path.display())),
        }
    }
    (paths, rejected)
}

/// Reads piped standard input for `-` (APP-09). Only a file or pipe is read:
/// a console would wait for typing that nobody knows is expected. Startup waits,
/// before any window exists, until the producer closes the pipe or
/// `MAX_STDIN_BYTES` arrive, but never longer than `STDIN_WAIT`: a producer that
/// does not finish gets its text so far and a notice instead of an invisible hang.
fn read_stdin() -> StdinText {
    if !bareline_platform_windows::cli::stdin_redirected() {
        return StdinText {
            text: String::new(),
            note: Some("Standard input was not redirected, so nothing was read.".into()),
        };
    }
    read_piped(std::io::stdin(), STDIN_WAIT)
}

/// How long startup waits for piped standard input to end.
const STDIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Reads `input` on a worker for at most `wait`. A reader that is still blocked
/// then is left behind; it stops at its next read.
fn read_piped(input: impl std::io::Read + Send + 'static, wait: std::time::Duration) -> StdinText {
    use std::sync::{Arc, Mutex, PoisonError};
    let received = Arc::new(Mutex::new(Some(Vec::new())));
    let buffer = received.clone();
    let (done, finished) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("bareline-stdin".into())
        .spawn(move || {
            let mut input = input;
            let mut chunk = vec![0; 64 * 1024];
            let result = loop {
                match input.read(&mut chunk) {
                    Ok(0) => break Ok(()),
                    Ok(read) => {
                        let mut guard = buffer.lock().unwrap_or_else(PoisonError::into_inner);
                        // Startup stopped waiting and took the text so far.
                        let Some(bytes) = guard.as_mut() else {
                            break Ok(());
                        };
                        bytes.extend_from_slice(&chunk[..read]);
                        if bytes.len() > MAX_STDIN_BYTES {
                            break Ok(());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => break Err(error),
                }
            };
            let _ = done.send(result);
        });
    let outcome = match spawned {
        Ok(_) => finished.recv_timeout(wait),
        Err(error) => Ok(Err(error)),
    };
    let bytes = received
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_default();
    match outcome {
        Ok(Ok(())) => decode_stdin(bytes, MAX_STDIN_BYTES),
        Ok(Err(error)) => StdinText {
            text: String::new(),
            note: Some(format!("Standard input could not be read: {error}")),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => stdin_cut_short(bytes, wait),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => StdinText {
            text: String::new(),
            note: Some("Standard input could not be read.".into()),
        },
    }
}

/// The text received before `wait` ran out, with the timeout notice followed by
/// any damage or truncation notice for that partial text.
fn stdin_cut_short(bytes: Vec<u8>, wait: std::time::Duration) -> StdinText {
    let decoded = decode_stdin(bytes, MAX_STDIN_BYTES);
    let mut note = format!(
        "Standard input was still open after {} seconds; only the text received by then was read.",
        wait.as_secs()
    );
    if let Some(damage) = decoded.note {
        note.push(' ');
        note.push_str(&damage);
    }
    StdinText {
        text: decoded.text,
        note: Some(note),
    }
}

/// UTF-8 (with or without a signature) or UTF-16 with a byte-order mark. Other
/// bytes are shown with replacement characters and a notice, never silently.
fn decode_stdin(mut bytes: Vec<u8>, limit: usize) -> StdinText {
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let utf16 = |bytes: &[u8], little: bool| {
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| {
                let pair = [pair[0], pair.get(1).copied().unwrap_or(0)];
                if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        let exact = bytes.len() % 2 == 0 && char::decode_utf16(units.iter().copied()).all(|unit| unit.is_ok());
        (String::from_utf16_lossy(&units), exact)
    };
    let (text, exact) = if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        utf16(rest, true)
    } else if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        utf16(rest, false)
    } else {
        let rest = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes[..]);
        match std::str::from_utf8(rest) {
            Ok(text) => (text.to_owned(), true),
            Err(_) => (String::from_utf8_lossy(rest).into_owned(), false),
        }
    };
    let note = match (truncated, exact) {
        (true, _) => Some(format!(
            "Standard input was longer than {} MB; only the beginning was read.",
            limit >> 20
        )),
        (false, false) => Some("Standard input was not valid UTF-8 or UTF-16; invalid bytes were replaced.".into()),
        (false, true) => None,
    };
    StdinText { text, note }
}

/// The marker is an empty file by convention, but only its presence as a file
/// matters: its content or size never fails the launch (APP-01).
fn portable_marker(directory: &Path, ledger: &mut StartupLedger) -> bool {
    ledger.record(StartupAction::ReadSettings);
    directory.join("bareline.portable").is_file()
}

/// Relative arguments resolve against the launch directory. A drive-relative
/// argument such as `C:notes.txt` uses that drive's current directory, as the
/// shell does, through GetFullPathNameW (`std::path::absolute`) (APP-17).
fn resolve_launch_path(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
    let joined = cwd.join(path);
    let resolved = if joined.is_absolute() {
        joined
    } else {
        std::path::absolute(&joined).map_err(|error| error.to_string())?
    };
    if !valid_launch_path(&resolved) {
        return Err("not a valid file path".into());
    }
    Ok(resolved)
}

/// The same limits the instance handoff enforces for every forwarded path.
fn valid_launch_path(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    path.is_absolute() && !units.is_empty() && !units.contains(&0) && units.len() <= 32767
}

fn legacy_root(mode: LaunchMode, roaming: Option<PathBuf>) -> Option<PathBuf> {
    (mode == LaunchMode::Installed).then_some(roaming).flatten()
}

fn validate_diagnostic_root(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let marker = std::fs::symlink_metadata(root.join(".bareline-diagnostic"))?;
    if !marker.is_file() || marker.file_type().is_symlink() || marker.len() != 0 {
        return Err("Diagnostic root requires an empty regular .bareline-diagnostic marker".into());
    }
    Ok(root)
}

fn select_mode(requested: LaunchMode, portable_marker: bool) -> LaunchMode {
    if matches!(requested, LaunchMode::Installed | LaunchMode::Diagnostic) && portable_marker {
        LaunchMode::Portable
    } else {
        requested
    }
}

fn profile_initialization(
    mode: LaunchMode,
    roaming: Option<PathBuf>,
    local: Option<PathBuf>,
    temp: PathBuf,
) -> Option<ProfileInitialization> {
    (mode == LaunchMode::Installed).then_some(ProfileInitialization { roaming, local, temp })
}

/// `bareline --diag handles` records the process handle counters so a leak shows
/// up as a growing number across runs. Sampled at startup and after each close.
pub(super) fn log_handle_counters(directory: Option<&Path>) {
    let Some(directory) = directory else { return };
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let (handles, gdi, user) = handle_counters();
    let line = format!(
        "handles pid={} process={handles} gdi={gdi} user={user}\n",
        std::process::id()
    );
    use std::io::Write;
    let handle_path = directory.join("handles.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&handle_path) {
        // Opt-in diagnostics must remain bounded during long-running sessions.
        if file.metadata().is_ok_and(|metadata| metadata.len() < 1024 * 1024) {
            let _ = file.write_all(line.as_bytes());
        }
    }
    fn sample(value: Option<bareline_platform::executor::ExecutorStats>) -> serde_json::Value {
        value.map_or_else(
            || serde_json::json!({"initialized":false}),
            |stats| {
                serde_json::json!({
                    "initialized":true,"workers":stats.workers,"running":stats.running,
                    "queued":stats.queued,"submitted":stats.submitted,"completed":stats.completed,
                    "rejected":stats.rejected,"peak_running":stats.peak_running
                })
            },
        )
    }
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let snapshot = serde_json::json!({"schema_version":1,"pid":std::process::id(),
        "sequence":SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed),
        "pools":{"task":sample(bareline_app::task::pool_stats()),
            "extensions":sample(super::extensions::worker_stats()),
            "recovery_retirement":sample(bareline_file_io::recovery_retirement::executor_stats())}});
    let path = directory.join("worker-queues.json");
    let stage = directory.join(format!(".worker-queues-{}.tmp", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).create_new(true).open(&stage) {
        let result = serde_json::to_writer(&mut file, &snapshot)
            .map_err(std::io::Error::other)
            .and_then(|()| file.sync_all());
        drop(file);
        if result.is_ok() {
            let _ = bareline_platform::LocalFileSystem::commit(
                &bareline_platform_windows::WindowsFileSystem,
                &stage,
                &path,
                path.exists(),
            );
        }
        let _ = std::fs::remove_file(stage);
    }
}

#[cfg(windows)]
fn handle_counters() -> (u32, u32, u32) {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    // SAFETY: pseudo handle for the current process; counters are plain outputs.
    unsafe {
        let process = GetCurrentProcess();
        let mut handles = 0u32;
        let _ = GetProcessHandleCount(process, &mut handles);
        (
            handles,
            GetGuiResources(process, GR_GDIOBJECTS),
            GetGuiResources(process, GR_USEROBJECTS),
        )
    }
}
#[cfg(not(windows))]
fn handle_counters() -> (u32, u32, u32) {
    (0, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_returns_typed_modes_without_preparing_paths() {
        let mut ledger = StartupLedger::default();
        let help = parse(&[OsString::from("--help")], &mut ledger).unwrap();
        assert_eq!(help.mode(), LaunchMode::Help);

        let version = parse(&[OsString::from("--version")], &mut ledger).unwrap();
        assert_eq!(version.mode(), LaunchMode::Version);

        let diagnostic = parse(&[OsString::from("--smoke")], &mut ledger).unwrap();
        assert_eq!(diagnostic.mode(), LaunchMode::Diagnostic);

        let performance = parse(
            &[
                OsString::from("--perf-workload"),
                OsString::from("launch"),
                OsString::from("--perf-root"),
                OsString::from("missing-root-is-not-probed-by-parse"),
            ],
            &mut ledger,
        )
        .unwrap();
        assert_eq!(performance.mode(), LaunchMode::Performance);

        let installed = parse(&[], &mut ledger).unwrap();
        assert_eq!(installed.mode(), LaunchMode::Installed);
    }

    #[test]
    fn parser_preserves_option_like_and_non_ascii_paths_after_separator() {
        let mut ledger = StartupLedger::default();
        let parsed = parse(
            &[
                OsString::from("--"),
                OsString::from("--version"),
                OsString::from("文書.txt"),
            ],
            &mut ledger,
        )
        .unwrap();
        assert_eq!(parsed.mode(), LaunchMode::Installed);
        assert_eq!(
            parsed.options.paths,
            [PathBuf::from("--version"), PathBuf::from("文書.txt")]
        );
    }

    #[test]
    fn parser_rejects_contradictory_and_invalid_modes() {
        for args in [
            vec!["--help", "--version"],
            vec!["--software", "--hardware"],
            vec!["--smoke", "--perf"],
            vec!["--diag"],
            vec!["--diag", "unknown"],
        ] {
            let mut ledger = StartupLedger::default();
            assert!(parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>(), &mut ledger).is_err());
        }
        for diagnostic in [["--smoke"], ["--text-prototype"], ["--perf"], ["--diag=handles"]] {
            let mut args = ["--perf-workload", "launch", "--perf-root", "unprepared-root"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            args.extend(diagnostic.into_iter().map(OsString::from));
            let mut ledger = StartupLedger::default();
            assert!(parse(&args, &mut ledger).is_err());
        }
        let mut ledger = StartupLedger::default();
        assert!(
            parse(
                &[
                    "--perf-workload",
                    "launch",
                    "--perf-root",
                    "unprepared-root",
                    "--diag",
                    "handles",
                ]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
                &mut ledger,
            )
            .is_err()
        );
    }

    #[test]
    fn only_installed_mode_owns_profile_initialization() {
        assert_eq!(select_mode(LaunchMode::Installed, true), LaunchMode::Portable);
        assert_eq!(select_mode(LaunchMode::Diagnostic, true), LaunchMode::Portable);
        for mode in [
            LaunchMode::Help,
            LaunchMode::Version,
            LaunchMode::Diagnostic,
            LaunchMode::Portable,
            LaunchMode::Performance,
        ] {
            assert!(profile_initialization(mode, None, None, PathBuf::from("temp")).is_none());
        }
        assert!(profile_initialization(LaunchMode::Installed, None, None, PathBuf::from("temp")).is_some());
        let installed = PathBuf::from("installed-profile");
        assert_eq!(
            legacy_root(LaunchMode::Installed, Some(installed.clone())),
            Some(installed.clone())
        );
        for mode in [LaunchMode::Portable, LaunchMode::Diagnostic, LaunchMode::Performance] {
            assert_eq!(legacy_root(mode, Some(installed.clone())), None);
        }
    }

    #[test]
    fn diagnostic_root_requires_an_owned_empty_regular_marker() {
        let root = std::env::temp_dir().join(format!(
            "bareline-diagnostic-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join(".bareline-diagnostic");
        assert!(validate_diagnostic_root(&root).is_err());
        std::fs::write(&marker, []).unwrap();
        assert_eq!(validate_diagnostic_root(&root).unwrap(), root.canonicalize().unwrap());
        std::fs::write(&marker, b"not-owned").unwrap();
        assert!(validate_diagnostic_root(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_marker_is_detected_by_presence_as_a_file() {
        let root = std::env::temp_dir().join(format!(
            "bareline-portable-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("bareline.portable");
        let mut ledger = StartupLedger::default();
        assert!(!portable_marker(&root, &mut ledger));
        std::fs::write(&marker, []).unwrap();
        assert!(portable_marker(&root, &mut ledger));
        // A marker saved by an editor with a newline still selects portable mode.
        std::fs::write(&marker, b"\r\n").unwrap();
        assert!(portable_marker(&root, &mut ledger));
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        assert!(!portable_marker(&root, &mut ledger));
        assert_eq!(ledger.validate(), Ok(()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_paths_resolve_drive_relative_arguments() {
        let cwd = PathBuf::from(r"D:\work");
        assert_eq!(
            resolve_launch_path(&cwd, Path::new("notes.txt")).unwrap(),
            PathBuf::from(r"D:\work\notes.txt")
        );
        assert_eq!(
            resolve_launch_path(&cwd, Path::new(r"E:\abs.txt")).unwrap(),
            PathBuf::from(r"E:\abs.txt")
        );
        // Previously `cwd.join("C:foo.txt")` stayed drive-relative and the
        // instance handoff rejected it, ending the launch without a window.
        let resolved = resolve_launch_path(&cwd, Path::new("C:foo.txt")).unwrap();
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(resolved.starts_with(r"C:\") && resolved.ends_with("foo.txt"));
    }

    #[test]
    fn installed_initialization_retains_its_completion_receipt() {
        let temp = std::env::temp_dir().join(format!(
            "bareline-initialization-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let pending = profile_initialization(LaunchMode::Installed, None, None, temp.clone());
        let mut runtime = ProfileInitializationRuntime::new(pending);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        assert!(
            runtime
                .schedule(std::sync::Arc::new(move || {
                    let _ = sender.send(());
                }))
                .unwrap()
        );
        receiver.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(runtime.pump());
        let completion = runtime.completion().unwrap().as_ref().unwrap();
        assert_eq!(completion.profile_root, None);
        assert!(completion.migration.as_ref().unwrap().items.is_empty());
        assert!(completion.authorities.items.is_empty());
        assert!(completion.migrated_settings.is_none());
        assert!(runtime.settled());
        runtime.retry().unwrap();
        assert!(!runtime.settled());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn initialization_worker_reads_the_migrated_settings_for_the_ui_thread() {
        let temp = std::env::temp_dir().join(format!(
            "bareline-initialization-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (roaming, local, scratch) = (temp.join("roaming"), temp.join("local"), temp.join("temp"));
        std::fs::create_dir_all(&roaming).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(roaming.join("settings.toml"), b"[editor]\nfont_size = 17\n").unwrap();
        let pending = profile_initialization(LaunchMode::Installed, Some(roaming), Some(local.clone()), scratch);
        let mut runtime = ProfileInitializationRuntime::new(pending);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        assert!(
            runtime
                .schedule(std::sync::Arc::new(move || {
                    let _ = sender.send(());
                }))
                .unwrap()
        );
        receiver.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert!(runtime.pump());
        let completion = runtime.completion().unwrap().as_ref().unwrap();
        assert!(local.join("settings.toml").is_file());
        // The worker hands over the parsed document, so reconciling it on the UI
        // thread reads nothing from disk (APP-12).
        assert!(
            matches!(completion.migrated_settings, Some(Ok(_))),
            "{:?}",
            completion.migrated_settings
        );
        drop(runtime);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn launch_opens_the_first_sixteen_paths_and_names_the_rest() {
        let mut ledger = StartupLedger::default();
        let args: Vec<_> = (0..20)
            .map(|index| OsString::from(format!("file-{index}.txt")))
            .collect();
        // More than 16 paths used to refuse the whole launch without a word (APP-09).
        let parsed = parse(&args, &mut ledger).unwrap();
        assert_eq!(parsed.mode(), LaunchMode::Installed);
        let (paths, rejected) = launch_paths(Path::new(r"D:\work"), parsed.options.paths);
        assert_eq!(paths.len(), 16);
        assert_eq!(paths[0], PathBuf::from(r"D:\work\file-0.txt"));
        assert_eq!(paths[15], PathBuf::from(r"D:\work\file-15.txt"));
        assert_eq!(rejected.len(), 4);
        assert!(rejected[0].starts_with("file-16.txt: "), "{rejected:?}");
        assert!(rejected.iter().all(|line| line.contains("first 16 files")));
    }

    #[test]
    fn notepad_plus_plus_flags_and_stdin_reach_the_launch() {
        let mut ledger = StartupLedger::default();
        let parsed = parse(
            &[
                "-multiInst",
                "-nosession",
                "-n12",
                "-c4",
                "-ro",
                "-notabbar",
                "-",
                "a.txt",
            ]
            .map(OsString::from),
            &mut ledger,
        )
        .unwrap();
        assert_eq!(parsed.mode(), LaunchMode::Installed);
        assert!(parsed.has_paths());
        let options = &parsed.options;
        assert!(options.stdin && options.new_instance && options.no_session && options.read_only);
        assert_eq!((options.line, options.column), (Some(12), Some(4)));
        assert_eq!(options.paths, [PathBuf::from("a.txt")]);
        // Piped text alone is a document to open; diagnostic modes refuse it.
        assert!(parse(&[OsString::from("-")], &mut ledger).unwrap().has_paths());
        assert!(parse(&["--smoke", "-"].map(OsString::from), &mut ledger).is_err());
        assert!(parse(&["--smoke", "a.txt"].map(OsString::from), &mut ledger).is_err());
        // After the separator `-` is a file name.
        let literal = parse(&["--", "-"].map(OsString::from), &mut ledger).unwrap();
        assert!(!literal.options.stdin);
        assert_eq!(literal.options.paths, [PathBuf::from("-")]);
    }

    #[test]
    fn piped_text_decodes_utf8_and_utf16_and_reports_damage() {
        let utf8 = decode_stdin(b"\xef\xbb\xbfcaf\xc3\xa9\n".to_vec(), 1024);
        assert_eq!(utf8.text, "caf\u{e9}\n");
        assert!(utf8.note.is_none());
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("hi \u{1F642}".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let decoded = decode_stdin(utf16, 1024);
        assert_eq!(decoded.text, "hi \u{1F642}");
        assert!(decoded.note.is_none());
        let damaged = decode_stdin(vec![b'a', 0xff, b'b'], 1024);
        assert_eq!(damaged.text, "a\u{fffd}b");
        assert!(damaged.note.is_some());
        let long = decode_stdin(b"abcdef".to_vec(), 4);
        assert_eq!(long.text, "abcd");
        assert!(long.note.unwrap().contains("only the beginning"));
    }

    #[test]
    fn piped_text_that_never_ends_does_not_hold_startup() {
        // A producer that closes its pipe: the whole text, no notice.
        let whole = read_piped(
            std::io::Cursor::new(b"piped\n".to_vec()),
            std::time::Duration::from_secs(60),
        );
        assert_eq!(whole.text, "piped\n");
        assert!(whole.note.is_none());
        /// Blocks like a pipe whose producer never finishes, until the test ends.
        struct Open(std::sync::mpsc::Receiver<()>);
        impl std::io::Read for Open {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(0)
            }
        }
        let (producer, pipe) = std::sync::mpsc::channel();
        let open = read_piped(Open(pipe), std::time::Duration::ZERO);
        assert!(open.text.is_empty());
        assert!(open.note.is_some_and(|note| note.contains("still open")));
        // The left-behind reader stops once its input ends.
        drop(producer);
        // Text cut off mid-sequence keeps its damage notice after the timeout one.
        let cut = stdin_cut_short(vec![b'a', 0xe2, 0x82], std::time::Duration::from_secs(10));
        assert_eq!(cut.text, "a\u{fffd}");
        let note = cut.note.unwrap();
        let damage = decode_stdin(vec![b'a', 0xe2, 0x82], 1024).note.unwrap();
        assert!(
            note.starts_with("Standard input was still open after 10 seconds"),
            "{note}"
        );
        assert!(note.ends_with(&damage), "{note}");
    }

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
