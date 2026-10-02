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
                    self.note_replaced(kept, self.editors[index].document_identity());
                    self.editors[index].set_read_only(read_only);
                    self.untitled_labels[index] = format!("{label} (loading)");
                    self.find.clear_source();
                }
                None => {
                    self.editors.push(loading);
                    self.files.push(None);
                    self.untitled_labels.push(format!("{label} (loading)"));
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
