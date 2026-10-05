// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_file_io::codecs::{Encoding, state::EncodingState};
impl Workspace {
    pub fn encoding_state(&self, index: usize) -> Option<EncodingState> {
        let editor = self.editors.get(index)?;
        let fallback = || {
            bareline_file_io::codecs::state::EncodingState::new(bareline_file_io::codecs::Detection {
                encoding: Encoding::Utf8,
                confidence: bareline_file_io::codecs::Confidence::Utf8Sample,
                bom: self
                    .tabs
                    .get(index)
                    .and_then(|tab| tab.file.as_ref())
                    .is_some_and(|file| file.bom),
                binary_warning: false,
                candidates: [None; 3],
            })
        };
        Some(match editor {
            WorkspaceEditor::Paged(editor) => editor.encoding_state()?,
            WorkspaceEditor::Resident(editor) => {
                bareline_file_io::codecs::state::metadata_encoding(editor.snapshot().metadata())
                    .or_else(|| {
                        self.tabs
                            .get(index)
                            .and_then(|tab| tab.file.as_ref())
                            .and_then(|file| file.encoding.as_ref())
                            .map(|encoding| encoding.state.clone())
                    })
                    .unwrap_or_else(fallback)
            }
        })
    }
    pub fn binary_warning_pending(&self, index: usize) -> bool {
        self.tabs
            .get(index)
            .and_then(|tab| tab.file.as_ref())
            .is_some_and(|file| !file.binary_accepted)
            && self.encoding_state(index).is_some_and(|state| state.binary_warning)
    }
    /// Text of the in-view notice for a binary-like document still awaiting a
    /// decision (UI-01). It names the file and never blocks other documents.
    pub fn binary_notice(&self, index: usize) -> Option<String> {
        if !self.binary_warning_pending(index) {
            return None;
        }
        let path = &self.tabs.get(index)?.file.as_ref()?.path;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Some(crate::encoding::binary_notice(&name))
    }
    /// "Encoding may be wrong" hint for an ambiguous detection (FIO-05), shown as
    /// the open's status message. It names the file and never blocks.
    pub fn encoding_hint(&self, index: usize) -> Option<String> {
        let hint = crate::encoding::detection_hint(&self.encoding_state(index)?)?;
        let path = &self.tabs.get(index)?.file.as_ref()?.path;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Some(format!("{name}: {hint}"))
    }
    pub fn encoding_convert(&mut self, index: usize, target: Encoding, bom: bool) -> Result<(), String> {
        if bom && target.bom().is_empty() {
            return Err("This encoding does not support a byte-order mark".into());
        }
        let mut state = self.encoding_state(index).ok_or("Encoding state is unavailable")?;
        state.convert_to(target);
        state.bom = bom;
        let editor = self.editors.get_mut(index).ok_or("Document unavailable")?;
        match editor {
            WorkspaceEditor::Resident(editor) => {
                let metadata = bareline_file_io::codecs::state::with_encoding(editor.snapshot().metadata(), &state)
                    .map_err(|error| error.to_string())?;
                editor.apply_document_metadata(metadata)
            }
            WorkspaceEditor::Paged(editor) => {
                let metadata = bareline_file_io::codecs::state::with_encoding(editor.snapshot().metadata(), &state)
                    .map_err(|error| error.to_string())?;
                editor.apply_document_metadata(metadata)
            }
        }
    }
    pub fn encoding_accept_binary(&mut self, index: usize, read_only: bool) -> Result<(), String> {
        let file = self
            .tabs
            .get_mut(index)
            .and_then(|tab| tab.file.as_mut())
            .ok_or("Document is unavailable")?;
        file.binary_accepted = true;
        self.editors
            .get_mut(index)
            .ok_or("Document is unavailable")?
            .set_read_only(read_only);
        Ok(())
    }
    pub(super) fn refresh_encoding_open(&mut self, index: usize) {
        if let Some(state) = self.encoding_state(index) {
            self.editors[index].viewport_mut().encoding_label = state.save_target.status_label(state.bom);
            if self.binary_warning_pending(index) {
                self.editors[index].set_read_only(true);
            }
        }
    }
    pub fn encoding_interpret(
        &mut self,
        index: usize,
        target: Encoding,
        discard_confirmed: bool,
    ) -> Result<(), String> {
        let editor = self.editors.get(index).ok_or("Document is unavailable")?;
        if editor.busy() || (editor.dirty() && !discard_confirmed) {
            return Err("Confirm discarding edits before interpreting original bytes".into());
        }
        let source = self
            .tabs
            .get(index)
            .and_then(|tab| tab.file.as_ref())
            .ok_or("No original byte source")?;
        if let WorkspaceEditor::Paged(paged) = editor {
            if self.interpreting_paged.is_some() {
                return Err("An interpretation is already running".into());
            }
            let captured = paged.snapshot().clone();
            let reload = PendingReload::capture(editor);
            let path = source.path.clone();
            let request = bareline_file_io::lifecycle::InterpretPagedRequest {
                source: paged.read_handle().original_store()?,
                target,
                path: path.clone(),
                fingerprint: source.fingerprint.clone(),
                cache: std::env::temp_dir().join("Bareline-transcode"),
                quota: self.transcode_quota_bytes,
                options: self.source_options(),
                bytes: self.bytes.clone(),
                history: self.history.clone(),
            };
            if !self.ensure_io() {
                return Err("File service unavailable".into());
            }
            let receiver = self
                .io
                .as_ref()
                .unwrap()
                .submit(IoRequest::InterpretPaged(Box::new(request)), self.notify.clone())
                .map_err(|_| "File queue is full")?;
            self.interpreting_paged = Some((captured, path.clone()));
            self.pending_io.push(PendingIo {
                completion: None,
                receiver,
                save: None,
                copy_only: false,
                open_path: Some(path),
                launch_request: None,
                recovery_restore_request: None,
                allow_duplicate: false,
                preview: None,
                reload: Some(reload),
                keep_failed_tab: false,
            });
            self.message = Some("Interpreting sealed original bytes…".into());
            return Ok(());
        }
        let encoding = source.encoding.clone().ok_or("Original bytes are unavailable")?;
        let request = bareline_file_io::lifecycle::InterpretRequest {
            source: encoding,
            target,
            dirty: editor.dirty(),
            discard_confirmed,
            bytes: self.bytes.clone(),
            history: self.history.clone(),
            path: source.path.clone(),
            fingerprint: source.fingerprint.clone(),
        };
        let mut captured = PendingReload::capture(editor);
        captured.interpret = Some(target);
        let path = source.path.clone();
        if !self.ensure_io() {
            return Err("File service unavailable".into());
        }
        let receiver = self
            .io
            .as_ref()
            .unwrap()
            .submit(IoRequest::Interpret(Box::new(request)), self.notify.clone())
            .map_err(|_| "File queue is full")?;
        self.pending_io.push(PendingIo {
            completion: None,
            receiver,
            save: None,
            copy_only: false,
            open_path: Some(path),
            launch_request: None,
            recovery_restore_request: None,
            allow_duplicate: false,
            preview: None,
            reload: Some(captured),
            keep_failed_tab: false,
        });
        self.message = Some("Interpreting retained original bytes…".into());
        Ok(())
    }
}

enum EolSource {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_editor_surface::paged_view::PagedReadHandle),
}
/// Changed terminators planned as separate edits before a paged conversion is staged.
const EOL_EDIT_CAP: usize = 131_072;
/// A planned conversion: bounded explicit edits, or past the edit cap one validated
/// source edit whose original and converted text live in an owned spill store.
enum EolPlan {
    Edits(bareline_document::EditTransaction),
    Source(Box<bareline_document::paged::PreparedSourceTransaction>),
}
pub(super) struct EolJob {
    cancellation: bareline_file_io::cancellation::Cancellation,
    receiver: std::sync::mpsc::Receiver<(EolSource, Result<EolPlan, String>)>,
}
impl Drop for EolJob {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
impl Workspace {
    pub fn encoding_eol(
        &mut self,
        index: usize,
        target: bareline_file_io::codecs::state::Eol,
        selection_only: bool,
    ) -> Result<(), String> {
        if self.eol_job.is_some() {
            return Err("A newline conversion is already running".into());
        }
        let editor = self.editors.get(index).ok_or("Document unavailable")?;
        if editor.busy() || editor.read_only() {
            return Err("Document is busy or read only".into());
        }
        // With no terminators there is no text to convert. Change the policy
        // used by the next Enter, as one undoable document metadata edit.
        if !selection_only
            && let WorkspaceEditor::Resident(editor) = &mut self.editors[index]
            && editor.snapshot().line_count() == 1
        {
            let mut values = editor.snapshot().metadata().values().clone();
            let label = match target {
                bareline_file_io::codecs::state::Eol::Lf => "lf",
                bareline_file_io::codecs::state::Eol::CrLf => "crlf",
                bareline_file_io::codecs::state::Eol::Cr => "cr",
            };
            values.insert("file.new_document_eol".into(), label.into());
            let metadata = bareline_document::DocumentMetadata::new(values)
                .map_err(|error| format!("The newline setting could not be applied: {error}."))?;
            return editor.apply_document_metadata(metadata);
        }
        // A paged selection is converted in whole-document offsets: folded or
        // hidden lines make viewport offsets differ from the source (PED-25).
        let (source, length, (anchor, caret)) = match &self.editors[index] {
            WorkspaceEditor::Resident(editor) => (
                EolSource::Resident(editor.snapshot().clone()),
                editor.snapshot().len(),
                (editor.selection.anchor, editor.selection.caret),
            ),
            WorkspaceEditor::Paged(editor) => {
                let (anchor, caret) = editor.global_selection();
                (
                    EolSource::Paged(editor.read_handle()),
                    editor.snapshot().len(),
                    (anchor.0, caret.0),
                )
            }
        };
        let selected = anchor.min(caret)..anchor.max(caret);
        if selection_only && selected.is_empty() {
            return Err("Select text before converting selection newlines".into());
        }
        let range = if selection_only { selected } else { 0..length };
        let budget = self.bytes.clone();
        let notify = self.notify.clone();
        let spill = EolSpill {
            cache: std::env::temp_dir().join("Bareline-owned-spill"),
            platform: self.file_system.clone(),
        };
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let cancellation = bareline_file_io::cancellation::Cancellation::default();
        let worker_cancel = cancellation.clone();
        std::thread::Builder::new()
            .name("encoding-eol".into())
            .spawn(move || {
                let result = plan_eol(&source, range, target, &budget, &worker_cancel, &spill);
                let _ = sender.send((source, result));
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.eol_job = Some(EolJob { receiver, cancellation });
        self.message = Some("Preparing newline conversion…".into());
        Ok(())
    }
    pub(super) fn pump_encoding(&mut self) -> bool {
        let Some(job) = &self.eol_job else {
            return false;
        };
        let result = match job.receiver.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.eol_job = None;
                self.message = Some("Newline worker stopped".into());
                return true;
            }
        };
        self.eol_job = None;
        let (source, result) = result;
        let applied = result.and_then(|plan| {
            for editor in &mut self.editors {
                match (editor, &source) {
                    (WorkspaceEditor::Resident(editor), EolSource::Resident(snapshot))
                        if editor.snapshot().same_document(snapshot) =>
                    {
                        return match plan {
                            EolPlan::Edits(transaction) => {
                                editor.apply_prepared(snapshot, transaction).map_err(str::to_owned)
                            }
                            EolPlan::Source(_) => Err("Newline conversion plan does not match the document".into()),
                        };
                    }
                    (WorkspaceEditor::Paged(editor), EolSource::Paged(handle))
                        if editor.snapshot().same_document(handle.snapshot()) =>
                    {
                        return match plan {
                            EolPlan::Edits(transaction) => editor.apply_prepared(handle.snapshot(), transaction),
                            EolPlan::Source(prepared) => editor.apply_prepared_source(handle.snapshot(), *prepared),
                        };
                    }
                    _ => {}
                }
            }
            Err("Document closed before newline conversion completed".into())
        });
        self.message = Some(match applied {
            Ok(()) => "Applying newline conversion…".into(),
            Err(error) => error,
        });
        true
    }
}
/// Owned spill location for paged conversions past the edit cap.
struct EolSpill {
    cache: std::path::PathBuf,
    platform: Arc<dyn LocalFileSystem>,
}
fn plan_eol(
    source: &EolSource,
    range: std::ops::Range<usize>,
    target: bareline_file_io::codecs::state::Eol,
    budget: &Budget,
    cancel: &bareline_file_io::cancellation::Cancellation,
    spill: &EolSpill,
) -> Result<EolPlan, String> {
    use bareline_document::TextOffset;
    let handle = match source {
        EolSource::Resident(snapshot) => {
            return bareline_file_io::codecs::state::plan_eol_conversion(
                snapshot,
                TextOffset(range.start)..TextOffset(range.end),
                target,
                EOL_EDIT_CAP,
            )
            .map(EolPlan::Edits)
            .map_err(|error| error.to_string());
        }
        EolSource::Paged(handle) => handle,
    };
    plan_paged_eol(handle.snapshot(), range, target, budget, cancel, spill, &mut |ticket| {
        handle.resolve_captured_page(ticket).map_err(|error| error.to_string())
    })
}
/// Visit the paged text of `range` in bounded windows as (absolute offset, text).
/// Bytes of `range` outside `required` are widened edge bytes (FIO-16): when one is
/// part of a multibyte scalar the aligned window trims it away, and it is skipped,
/// as it can never be a terminator. Every byte of `required` must be visited.
fn visit_eol_windows(
    snapshot: &bareline_document::paged::PagedSnapshot,
    range: std::ops::Range<usize>,
    required: std::ops::Range<usize>,
    budget: &Budget,
    cancel: &bareline_file_io::cancellation::Cancellation,
    resolve: &mut dyn FnMut(bareline_document::source::PageTicket) -> Result<bool, String>,
    mut visit: impl FnMut(usize, &str) -> Result<(), String>,
) -> Result<(), String> {
    use bareline_document::{TextOffset, paged::WindowPoll};
    let mut offset = range.start;
    while offset < range.end {
        cancel.check().map_err(|_| "Newline conversion cancelled")?;
        let mut request = snapshot
            .begin_viewport(TextOffset(offset), (range.end - offset).min(64 * 1024), budget)
            .map_err(|error| error.to_string())?;
        let window = loop {
            cancel.check().map_err(|_| "Newline conversion cancelled")?;
            match request.poll() {
                WindowPoll::Ready(window) => break window,
                WindowPoll::Pending(ticket) => {
                    if !resolve(ticket)? {
                        std::thread::yield_now();
                    }
                }
                _ => return Err("Newline source unavailable".into()),
            }
        };
        let start = window.range().start.0;
        let end = window.range().end.0.min(range.end);
        if end <= offset {
            // The widened trailing byte may start a multibyte scalar the aligned
            // window trims away; it is then no terminator.
            if offset >= required.end {
                break;
            }
            return Err("Newline source made no progress".into());
        }
        if start > offset && start <= required.start {
            // The widened leading byte continued a scalar and was trimmed.
            offset = start;
        }
        let text = offset
            .checked_sub(start)
            .and_then(|local| window.text().get(local..end - start))
            .ok_or("Newline source window is misaligned")?;
        visit(offset, text)?;
        offset = end;
    }
    Ok(())
}
/// Up to the edit cap each changed terminator is one edit. Past it, the changed span
/// is staged once as original and converted text in an owned spill store and applied
/// as one validated source edit, so any number of line endings converts.
fn plan_paged_eol(
    snapshot: &bareline_document::paged::PagedSnapshot,
    range: std::ops::Range<usize>,
    target: bareline_file_io::codecs::state::Eol,
    budget: &Budget,
    cancel: &bareline_file_io::cancellation::Cancellation,
    spill: &EolSpill,
    resolve: &mut dyn FnMut(bareline_document::source::PageTicket) -> Result<bool, String>,
) -> Result<EolPlan, String> {
    use bareline_document::{
        Edit, EditTransaction, TextOffset,
        paged::{OwnedTextRange, SourceEdit, SourceTransactionPoll},
    };
    use bareline_file_io::codecs::state::{Eol, convert_eol};
    // FIO-16: as in `plan_eol_conversion`, scan one byte past each edge and convert
    // only terminators overlapping the requested range, so a CRLF is never split.
    let requested = range;
    let range = if requested.is_empty() {
        requested.clone()
    } else {
        requested.start.saturating_sub(1)..(requested.end + 1).min(snapshot.len())
    };
    let mut edits = Vec::new();
    let mut changed: Option<std::ops::Range<usize>> = None;
    let mut coalesce = false;
    let mut cr = None;
    let mut emit = |start: usize, len: usize, current: Eol| {
        if current != target && start < requested.end && start + len > requested.start {
            changed.get_or_insert(start..start).end = start + len;
            if !coalesce && edits.len() >= EOL_EDIT_CAP {
                coalesce = true;
                edits = Vec::new();
            }
            if !coalesce {
                edits.push(Edit {
                    range: TextOffset(start)..TextOffset(start + len),
                    insert: target.text().into(),
                });
            }
        }
    };
    visit_eol_windows(
        snapshot,
        range,
        requested.clone(),
        budget,
        cancel,
        resolve,
        |offset, text| {
            for (local, byte) in text.bytes().enumerate() {
                let at = offset + local;
                if let Some(previous) = cr.take() {
                    if byte == b'\n' {
                        emit(previous, 2, Eol::CrLf);
                        continue;
                    }
                    emit(previous, 1, Eol::Cr);
                }
                match byte {
                    b'\r' => cr = Some(at),
                    b'\n' => emit(at, 1, Eol::Lf),
                    _ => {}
                }
            }
            Ok(())
        },
    )?;
    if let Some(previous) = cr {
        emit(previous, 1, Eol::Cr);
    }
    let Some(span) = changed.filter(|_| coalesce) else {
        return Ok(EolPlan::Edits(EditTransaction {
            base_revision: snapshot.revision,
            edits,
        }));
    };
    let mut store = bareline_file_io::owned_store::StreamingStoreBuilder::new(
        &spill.cache,
        20u64 << 30,
        spill.platform.clone(),
        bareline_file_io::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 64 * 1024,
            page_cache_bytes: 256 * 1024,
        },
        budget.clone(),
        cancel.clone(),
    )
    .map_err(|error| error.to_string())?;
    visit_eol_windows(
        snapshot,
        span.clone(),
        span.clone(),
        budget,
        cancel,
        resolve,
        |_, text| store.append_utf8(text).map(|_| ()).map_err(|error| error.to_string()),
    )?;
    let inverse = 0..store.len();
    let mut pending_cr = false;
    let mut converted = String::new();
    visit_eol_windows(
        snapshot,
        span.clone(),
        span.clone(),
        budget,
        cancel,
        resolve,
        |_, text| {
            converted.clear();
            convert_eol(text, target, &mut pending_cr, false, &mut converted);
            store
                .append_utf8(&converted)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )?;
    converted.clear();
    convert_eol("", target, &mut pending_cr, true, &mut converted);
    store.append_utf8(&converted).map_err(|error| error.to_string())?;
    let inserted = inverse.end..store.len();
    let backing = store.finish().map_err(|error| error.to_string())?;
    let edit = SourceEdit {
        range: TextOffset(span.start)..TextOffset(span.end),
        inverse: OwnedTextRange {
            source: backing.clone(),
            range: inverse,
        },
        inserted: OwnedTextRange {
            source: backing,
            range: inserted,
        },
    };
    let mut request = snapshot
        .prepare_source_transaction(vec![edit], Default::default(), budget.clone())
        .map_err(|error| error.to_string())?;
    loop {
        if cancel.check().is_err() {
            request.cancel();
            return Err("Newline conversion cancelled".into());
        }
        match request.poll() {
            SourceTransactionPoll::Ready(prepared) => return Ok(EolPlan::Source(Box::new(prepared))),
            SourceTransactionPoll::Progress => {}
            SourceTransactionPoll::Pending(ticket) => {
                if !request.resolve_owned(ticket).map_err(|error| error.to_string())? && !resolve(ticket)? {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Newline conversion source validation failed".into()),
        }
    }
}

impl Workspace {
    pub fn encoding_failure(&self, index: usize) -> Option<EncodingFailure> {
        match self.editors.get(index)? {
            WorkspaceEditor::Paged(editor) => editor.encoding_failure(),
            WorkspaceEditor::Resident(editor) => self
                .encoding_failures
                .iter()
                .rev()
                .find(|failure| failure.same_document(editor.snapshot().identity_token()))
                .cloned(),
        }
    }
    pub fn encoding_reveal_failure(&mut self, index: usize) -> Result<(), String> {
        let failure = self
            .encoding_failure(index)
            .ok_or("No encoding failure for this document")?;
        let editor = self.editors.get_mut(index).ok_or("Document unavailable")?;
        let identity = match editor {
            WorkspaceEditor::Resident(editor) => editor.snapshot().identity_token(),
            WorkspaceEditor::Paged(editor) => editor.snapshot().identity_token(),
        };
        if !failure.matches(identity) {
            return Err("Document changed since this failure; save again to locate the current problem".into());
        }
        if editor.busy() {
            return Err("Document is busy".into());
        }
        match editor {
            WorkspaceEditor::Resident(editor) => {
                editor.enqueue(Input::SetCaret(failure.range.start.0, false));
                editor.enqueue(Input::SetCaret(failure.range.end.0, true));
                Ok(())
            }
            WorkspaceEditor::Paged(editor) => editor.restore_selection(failure.range.start, failure.range.end),
        }
    }
    /// Full-file status. Revision-keyed worker results can never describe a newer root.
    pub fn encoding_eol_label(&self, index: usize) -> String {
        let Some(editor) = self.editors.get(index) else {
            return "Computing".into();
        };
        let WorkspaceEditor::Paged(editor) = editor else {
            return editor.snapshot().eol_label().into();
        };
        if let Some(label) = editor.initial_eol_label() {
            return label.into();
        }
        let identity = editor.snapshot().identity_token();
        let mut tracker = self.eol_status.borrow_mut();
        if let Some(pending) = &tracker.pending {
            match pending.receiver.try_recv() {
                Ok((key, result)) => {
                    tracker.pending = None;
                    if tracker.ready.len() == 32 {
                        tracker.ready.remove(0);
                    }
                    tracker
                        .ready
                        .push((key, result.unwrap_or_else(|_| "Unavailable".into())));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let key = pending.identity;
                    tracker.pending = None;
                    if tracker.ready.len() == 32 {
                        tracker.ready.remove(0);
                    }
                    tracker.ready.push((key, "Unavailable".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((_, label)) = tracker.ready.iter().rev().find(|(key, _)| *key == identity) {
            return label.clone();
        }
        if tracker
            .pending
            .as_ref()
            .is_none_or(|pending| pending.identity != identity)
        {
            tracker.pending = None;
            let source = editor.read_handle();
            let budget = self.bytes.clone();
            let notify = self.notify.clone();
            let cancellation = bareline_file_io::cancellation::Cancellation::default();
            let cancel = cancellation.clone();
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            if std::thread::Builder::new()
                .name("encoding-eol-status".into())
                .spawn(move || {
                    let result = scan_eol(&source, &budget, &cancel).map(|state| state.label().to_owned());
                    let _ = sender.send((identity, result));
                    notify();
                })
                .is_err()
            {
                return "Unavailable".into();
            }
            tracker.pending = Some(EolStatusPending {
                identity,
                cancellation,
                receiver,
            });
        }
        "Computing".into()
    }
}
#[derive(Default)]
pub(super) struct EolTracker {
    ready: Vec<((u64, u64), String)>,
    pending: Option<EolStatusPending>,
}
struct EolStatusPending {
    identity: (u64, u64),
    cancellation: bareline_file_io::cancellation::Cancellation,
    receiver: std::sync::mpsc::Receiver<((u64, u64), Result<String, String>)>,
}
impl Drop for EolStatusPending {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
fn scan_eol(
    handle: &bareline_editor_surface::paged_view::PagedReadHandle,
    budget: &Budget,
    cancel: &bareline_file_io::cancellation::Cancellation,
) -> Result<bareline_file_io::codecs::state::EolState, String> {
    use bareline_document::{TextOffset, paged::WindowPoll};
    let snapshot = handle.snapshot();
    let mut offset = 0;
    let mut eol = bareline_file_io::codecs::state::EolState::default();
    while offset < snapshot.len() {
        cancel.check().map_err(|_| "EOL scan cancelled")?;
        let mut request = snapshot
            .begin_viewport(TextOffset(offset), (snapshot.len() - offset).min(64 * 1024), budget)
            .map_err(|error| error.to_string())?;
        let window = loop {
            cancel.check().map_err(|_| "EOL scan cancelled")?;
            match request.poll() {
                WindowPoll::Ready(window) => break window,
                WindowPoll::Pending(ticket) => {
                    if !handle
                        .resolve_captured_page(ticket)
                        .map_err(|error| error.to_string())?
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                _ => return Err("EOL source unavailable".into()),
            }
        };
        let range = window.range();
        if range.end.0 <= offset || range.start.0 > offset {
            return Err("EOL scan made no progress".into());
        }
        let text = &window.text()[offset - range.start.0..];
        eol.push(text, false);
        offset = range.end.0;
    }
    eol.push("", true);
    Ok(eol)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::PagedFileSystem;

    #[test]
    fn paged_conversion_of_300k_crlf_lines_stages_one_source_edit() {
        use bareline_document::{TextOffset, paged::WindowPoll};
        use bareline_file_io::{
            cancellation::Cancellation,
            codecs::{disk::DiskOptions, state::Eol},
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!("bareline-eol-paged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("crlf.txt");
        std::fs::write(&path, "x\r\n".repeat(300_000)).unwrap();
        let platform: Arc<dyn LocalFileSystem> = Arc::new(PagedFileSystem);
        let budget = Budget::new(64 << 20);
        let TranscodeOutcome::Complete(mut opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(64 << 20),
                cache: root.join("cache"),
                options: DiskOptions {
                    temp_quota_bytes: 1 << 30,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..Default::default()
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture open failed")
        };
        let snapshot = opened.transcoded.document.snapshot();
        let spill = EolSpill {
            cache: root.join("spill"),
            platform,
        };
        let plan = plan_paged_eol(
            &snapshot,
            0..snapshot.len(),
            Eol::Lf,
            &budget,
            &Cancellation::default(),
            &spill,
            &mut |ticket| {
                opened
                    .transcoded
                    .source
                    .read_page(ticket)
                    .map(|()| true)
                    .map_err(|error| error.to_string())
            },
        )
        .unwrap();
        let EolPlan::Source(prepared) = plan else {
            panic!("line endings past the edit cap must stage one source edit")
        };
        opened.transcoded.document.commit_source_transaction(*prepared).unwrap();
        let after = opened.transcoded.document.snapshot();
        let mut read = after
            .begin_read(TextOffset(0)..TextOffset(after.len()), after.len(), &budget)
            .unwrap();
        let text = loop {
            match read.poll() {
                WindowPoll::Ready(window) => break window.text().to_owned(),
                WindowPoll::Pending(ticket) => {
                    if !after.resolve_owned(ticket).unwrap() {
                        opened.transcoded.source.read_page(ticket).unwrap();
                    }
                }
                _ => panic!("converted text unavailable"),
            }
        };
        assert_eq!(text, "x\n".repeat(300_000));
        drop(read);
        drop(after);
        drop(snapshot);
        drop(opened);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn paged_selection_edges_skip_trimmed_multibyte_neighbors() {
        use bareline_document::TextOffset;
        use bareline_file_io::{
            cancellation::Cancellation,
            codecs::{disk::DiskOptions, state::Eol},
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        // Bytes: é 0-1, CR 2, LF 3, é 4-5. Widening 2..4 by one byte reaches the middle
        // of each `é`; the aligned windows trim those bytes and the CRLF still converts.
        let root = std::env::temp_dir().join(format!("bareline-eol-paged-edges-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("edges.txt");
        std::fs::write(&path, "é\r\né").unwrap();
        let platform: Arc<dyn LocalFileSystem> = Arc::new(PagedFileSystem);
        let budget = Budget::new(64 << 20);
        let TranscodeOutcome::Complete(mut opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(64 << 20),
                cache: root.join("cache"),
                options: DiskOptions {
                    temp_quota_bytes: 1 << 30,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..Default::default()
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture open failed")
        };
        let snapshot = opened.transcoded.document.snapshot();
        let spill = EolSpill {
            cache: root.join("spill"),
            platform,
        };
        for (range, expected) in [
            (2..4, vec![TextOffset(2)..TextOffset(4)]),
            (3..4, vec![TextOffset(2)..TextOffset(4)]),
            (4..6, vec![]),
        ] {
            let plan = plan_paged_eol(
                &snapshot,
                range.clone(),
                Eol::Lf,
                &budget,
                &Cancellation::default(),
                &spill,
                &mut |ticket| {
                    opened
                        .transcoded
                        .source
                        .read_page(ticket)
                        .map(|()| true)
                        .map_err(|error| error.to_string())
                },
            )
            .unwrap_or_else(|error| panic!("{range:?}: {error}"));
            let EolPlan::Edits(transaction) = plan else {
                panic!("a single terminator stays an explicit edit")
            };
            assert_eq!(
                transaction
                    .edits
                    .iter()
                    .map(|edit| edit.range.clone())
                    .collect::<Vec<_>>(),
                expected,
                "{range:?}"
            );
            assert!(transaction.edits.iter().all(|edit| edit.insert == "\n"));
        }
        drop(snapshot);
        drop(opened);
        let _ = std::fs::remove_dir_all(&root);
    }
}
