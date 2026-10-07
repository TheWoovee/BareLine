// SPDX-License-Identifier: MPL-2.0
//! The file-operation queue: each completion goes to its handler in
//! submission order (ARC-01).
use super::*;
use std::sync::mpsc::TryRecvError;
impl Workspace {
    /// Settle every queued file operation whose worker answered, in order.
    pub(super) fn pump_io(&mut self) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.pending_io.len() {
            changed |= self.show_loading_prefix(i);
            let received = if let Some(completion) = self.pending_io[i].completion.take() {
                Ok(completion)
            } else {
                self.pending_io[i].receiver.try_recv()
            };
            let result = match received {
                Ok(result) => result,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => {
                    let failed = self.pending_io.remove(i);
                    self.fail_stopped_io(failed);
                    changed = true;
                    continue;
                }
            };
            let target = match &result {
                IoCompletion::Open(Ok(opened)) => Some((opened.path.clone(), opened.fingerprint.identity)),
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(opened)) => {
                    Some((opened.path.clone(), opened.fingerprint.identity))
                }
                IoCompletion::Save(Ok(saved)) if !self.pending_io[i].copy_only => self.pending_io[i]
                    .save
                    .as_ref()
                    .map(|(_, path, _)| (path.clone(), saved.fingerprint.identity)),
                _ => None,
            };
            let admission = if self.pending_io[i].allow_duplicate {
                None
            } else if let Some((path, identity)) = target {
                match self.replacement_registry.try_register(path, &identity) {
                    Ok(lease) => Some(lease),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        self.pending_io[i].completion = Some(result);
                        i += 1;
                        continue;
                    }
                    Err(error) => {
                        let pending = self.pending_io.remove(i);
                        self.fail_admission(pending, error);
                        changed = true;
                        continue;
                    }
                }
            } else {
                None
            };
            match self.gate_reload(i, &result) {
                ReloadGate::Apply => {}
                ReloadGate::Defer => {
                    self.pending_io[i].completion = Some(result);
                    i += 1;
                    continue;
                }
                ReloadGate::Abandon => {
                    let abandoned = self.pending_io.remove(i);
                    if self
                        .interpreting_paged
                        .as_ref()
                        .is_some_and(|(_, path)| abandoned.open_path.as_ref() == Some(path))
                    {
                        self.interpreting_paged = None;
                    }
                    changed = true;
                    continue;
                }
            }
            let pending = self.pending_io.remove(i);
            changed = true;
            self.complete_io(pending, result, admission);
        }
        changed
    }
    /// A resident open's first prefix shows as a read-only loading tab, in
    /// place of the tab an earlier attempt kept.
    fn show_loading_prefix(&mut self, i: usize) -> bool {
        if let Ok(prefix) = self.pending_io[i].receiver.try_prefix()
            && self.pending_io[i].reload.is_none()
        {
            let label = self.pending_io[i]
                .open_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map_or_else(|| "File".into(), |name| name.to_string_lossy().into_owned());
            let loading: WorkspaceEditor = EditorSurface::loading(prefix.clone(), self.notify.clone()).into();
            let shown = prefix.identity_token().0;
            // A tab kept from an earlier attempt shows the new preview in place.
            match self.preview_index(self.pending_io[i].preview.as_ref()) {
                Some(index) => {
                    let read_only = self.editors[index].viewport().user_read_only;
                    let kept = self.editors[index].document_identity();
                    self.retired.push(std::mem::replace(&mut self.editors[index], loading));
                    self.note_tab_replaced(index, kept);
                    self.editors[index].set_read_only(read_only);
                    self.tabs[index].label = format!("{label} (loading)");
                    let _ = self.apply_lifecycle(index, LifecycleEvent::LoadStarted);
                    self.find.clear_source();
                }
                None => {
                    self.push_tab(loading, None, format!("{label} (loading)"), LifecycleEvent::LoadStarted);
                }
            }
            self.pending_io[i].preview = Some(prefix);
            if let Some(request) = self.pending_io[i].launch_request {
                self.show_activation(request, shown);
            }
            true
        } else {
            false
        }
    }
    /// Hand one received completion to its handler.
    fn complete_io(
        &mut self,
        pending: PendingIo,
        result: IoCompletion,
        admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        match result {
            IoCompletion::Open(Ok(opened)) => self.complete_open(pending, opened, admission),
            IoCompletion::Save(Ok(saved)) => self.complete_save(pending, saved, admission),
            IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(opened)) => {
                self.complete_transcode(pending, opened, admission)
            }
            IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Paused(paused)) => {
                self.complete_transcode_paused(pending, paused)
            }
            IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Failed(error)) => {
                self.complete_transcode_failed(pending, error)
            }
            IoCompletion::ResidentSpilled { captured, result } => self.complete_spill(captured, result),
            IoCompletion::Open(Err(FileError::StreamingRequired)) => self.complete_streaming_required(pending),
            IoCompletion::Open(Err(FileError::Io(error)))
                if error.kind() == std::io::ErrorKind::NotFound
                    && pending.open_path.is_some()
                    && self
                        .missing_launches
                        .iter()
                        .any(|(request, _)| Some(*request) == pending.launch_request) =>
            {
                self.complete_missing_launch(pending)
            }
            IoCompletion::Open(Err(error)) | IoCompletion::Save(Err(error)) => self.complete_io_failure(pending, error),
            IoCompletion::SaveRecoveryInspection { result, .. } => self.complete_queued_save_recovery(result),
            IoCompletion::SaveCleanupRetried { .. } => {}
        }
    }
    /// The file worker stopped before answering: every waiter fails.
    pub(super) fn fail_stopped_io(&mut self, failed: PendingIo) {
        let error = "File worker stopped.".to_string();
        self.message = Some(error.clone());
        self.settle_save(failed.save.as_ref().map(|(tab, _, _)| *tab), false);
        if self.spill_pending {
            self.spill_pending = false;
            self.spill_paused = true;
        }
        self.resume_abandoned_reload(failed.reload.as_ref());
        self.settle_failed_open(
            failed.preview.as_ref(),
            failed.open_path.clone(),
            failed.keep_failed_tab,
            &error,
        );
        self.record_launch_open(failed.launch_request, Err(error));
        self.record_recovery_restore(
            failed.recovery_restore_request,
            Err("Recovery restore worker stopped.".into()),
        );
    }
    /// The finished file could not be registered as open, so it is not shown.
    pub(super) fn fail_admission(&mut self, pending: PendingIo, error: std::io::Error) {
        self.settle_save(pending.save.as_ref().map(|(tab, _, _)| *tab), false);
        self.resume_abandoned_reload(pending.reload.as_ref());
        let error = format!("File admission failed: {error}");
        self.settle_failed_open(
            pending.preview.as_ref(),
            pending.open_path.clone(),
            pending.keep_failed_tab,
            &error,
        );
        self.message = Some(error.clone());
        self.record_launch_open(pending.launch_request, Err(error.clone()));
        self.record_recovery_restore(pending.recovery_restore_request, Err(error));
    }
    /// A resident open too large for memory continues as a paged open in the
    /// same tab.
    pub(super) fn complete_streaming_required(&mut self, pending: PendingIo) {
        let launch_request = pending.launch_request;
        match pending.open_path {
            // The loading tab stays while the paged fallback runs (FIO-01).
            // Interpret As keeps its chosen encoding on the paged path.
            Some(path) => {
                let interpret = pending.reload.as_ref().and_then(|reload| reload.interpret);
                let request = self.paged_open_request(path.clone(), interpret);
                let before = self.pending_io.len();
                self.submit_paged_open(
                    request,
                    path,
                    launch_request,
                    pending.allow_duplicate,
                    pending.preview,
                    pending.keep_failed_tab,
                );
                // A reload's paged result replaces its captured tab in place.
                if self.pending_io.len() > before {
                    self.pending_io.last_mut().unwrap().reload = pending.reload;
                } else {
                    // The paged fallback never started, so the reload is
                    // abandoned: recovery stays with the text still open.
                    self.resume_abandoned_reload(pending.reload.as_ref());
                }
            }
            None => self.discard_preview(pending.preview.as_ref()),
        }
    }
    /// A tracked launch path that does not exist becomes a new document or the
    /// plain not-found notice (APP-09, APP-21).
    fn complete_missing_launch(&mut self, pending: PendingIo) {
        let launch_request = pending.launch_request;
        self.discard_preview(pending.preview.as_ref());
        let path = pending.open_path.clone().unwrap();
        let create = self
            .missing_launches
            .iter()
            .any(|(request, create)| Some(*request) == launch_request && *create);
        let result = if create {
            self.new_document_for_missing(path)
        } else {
            let error = missing_file_message(&path);
            self.message = Some(error.clone());
            Err(error)
        };
        self.record_launch_open(launch_request, result);
    }
    /// An open or save that failed.
    pub(super) fn complete_io_failure(&mut self, pending: PendingIo, error: FileError) {
        let launch_request = pending.launch_request;
        if let FileError::EncodingAt(failure) = &error {
            if self.encoding_failures.len() == 32 {
                self.encoding_failures.remove(0);
            }
            self.encoding_failures.push(failure.clone());
        }
        if let Some(conflict) = error.save_conflict() {
            self.record_save_conflict(conflict);
        }
        let own = pending
            .save
            .as_ref()
            .and_then(|(tab, _, _)| self.tab_index(*tab))
            .and_then(|index| self.path(index))
            .map(std::path::Path::to_path_buf);
        Self::record_deleted_destination(&mut self.deleted_destinations, &error, own.as_deref());
        self.settle_save(pending.save.as_ref().map(|(tab, _, _)| *tab), false);
        self.resume_abandoned_reload(pending.reload.as_ref());
        // A user-cancelled open drops its tab; any other failure keeps it.
        let cancelled = matches!(error, FileError::Cancelled);
        let error = file_error(error);
        self.settle_failed_open(
            pending.preview.as_ref(),
            pending.open_path.clone(),
            pending.keep_failed_tab && !cancelled,
            &error,
        );
        self.message = Some(error.clone());
        self.record_launch_open(launch_request, Err(error));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{PagedFileSystem, pending_io};

    fn fixture() -> Workspace {
        Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap()
    }

    #[test]
    fn a_stopped_file_worker_fails_every_waiter_and_pauses_spilling() {
        let mut workspace = fixture();
        let path = std::env::temp_dir().join("bareline-handler-stopped.txt");
        let mut failed = pending_io(&mut workspace);
        failed.open_path = Some(path.clone());
        failed.keep_failed_tab = true;
        failed.launch_request = Some(7);
        failed.recovery_restore_request = Some(9);
        workspace.spill_pending = true;
        workspace.fail_stopped_io(failed);
        assert_eq!(workspace.message.as_deref(), Some("File worker stopped."));
        assert!(!workspace.spill_pending);
        assert!(workspace.spill_paused, "automatic spilling waits for an explicit retry");
        assert_eq!(workspace.editors.len(), 1, "the user's open keeps its error tab");
        assert_eq!(workspace.failed_open(0), Some((path.as_path(), "File worker stopped.")));
        assert_eq!(
            workspace.take_launch_open_outcomes(),
            [LaunchOpenOutcome::Failed {
                request_id: 7,
                error: "File worker stopped.".into()
            }]
        );
        assert_eq!(
            workspace.take_recovery_restore_outcome(9),
            Some(RecoveryRestoreOutcome::Failed {
                request_id: 9,
                error: "Recovery restore worker stopped.".into()
            })
        );
    }

    #[test]
    fn an_unregistered_file_fails_its_open_and_its_restore() {
        let mut workspace = fixture();
        let mut pending = pending_io(&mut workspace);
        pending.open_path = Some(std::env::temp_dir().join("bareline-handler-admission.txt"));
        pending.launch_request = Some(3);
        pending.recovery_restore_request = Some(4);
        workspace.fail_admission(pending, std::io::Error::other("registry closed"));
        let error = "File admission failed: registry closed".to_string();
        assert_eq!(workspace.message.as_deref(), Some(error.as_str()));
        assert!(workspace.editors.is_empty(), "only a user open keeps a failed tab");
        assert_eq!(
            workspace.take_launch_open_outcomes(),
            [LaunchOpenOutcome::Failed {
                request_id: 3,
                error: error.clone()
            }]
        );
        assert_eq!(
            workspace.take_recovery_restore_outcome(4),
            Some(RecoveryRestoreOutcome::Failed { request_id: 4, error })
        );
    }

    #[test]
    fn a_cancelled_open_drops_its_tab_while_other_failures_keep_one() {
        let mut workspace = fixture();
        let path = std::env::temp_dir().join("bareline-handler-failure.txt");
        let mut cancelled = pending_io(&mut workspace);
        cancelled.open_path = Some(path.clone());
        cancelled.keep_failed_tab = true;
        cancelled.launch_request = Some(11);
        workspace.complete_io_failure(cancelled, FileError::Cancelled);
        assert!(workspace.editors.is_empty());
        assert_eq!(workspace.message, Some(file_error(FileError::Cancelled)));
        assert!(matches!(
            workspace.take_launch_open_outcomes().as_slice(),
            [LaunchOpenOutcome::Failed { request_id: 11, .. }]
        ));

        let mut failed = pending_io(&mut workspace);
        failed.open_path = Some(path.clone());
        failed.keep_failed_tab = true;
        workspace.complete_io_failure(failed, FileError::Io(std::io::Error::other("disk gone")));
        let error = workspace.message.clone().expect("the failure is reported");
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.failed_open(0), Some((path.as_path(), error.as_str())));
    }

    #[test]
    fn a_paged_fallback_without_a_path_drops_its_loading_tab() {
        let mut workspace = fixture();
        workspace.new_document().unwrap();
        let mut pending = pending_io(&mut workspace);
        pending.preview = Some(workspace.editors[0].snapshot().clone());
        workspace.complete_streaming_required(pending);
        assert!(workspace.editors.is_empty());
        assert!(workspace.pending_io.is_empty(), "nothing was resubmitted");
    }

    #[test]
    fn stored_completions_settle_in_submission_order() {
        let mut workspace = fixture();
        assert!(!workspace.pump_io(), "an empty queue changes nothing");
        let completions = [
            (21, IoCompletion::Open(Err(FileError::Cancelled))),
            (
                22,
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Failed(
                    FileError::Cancelled,
                )),
            ),
        ];
        for (request_id, completion) in completions {
            let mut pending = pending_io(&mut workspace);
            pending.launch_request = Some(request_id);
            pending.completion = Some(completion);
            workspace.pending_io.push(pending);
        }
        assert!(workspace.pump_io());
        assert!(workspace.pending_io.is_empty());
        let settled: Vec<u64> = workspace
            .take_launch_open_outcomes()
            .into_iter()
            .map(|outcome| match outcome {
                LaunchOpenOutcome::Failed { request_id, .. } => request_id,
                LaunchOpenOutcome::Opened { request_id, .. } => panic!("request {request_id} cannot open"),
            })
            .collect();
        assert_eq!(settled, [21_u64, 22]);
    }
}
