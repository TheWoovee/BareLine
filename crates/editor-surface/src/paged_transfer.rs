// SPDX-License-Identifier: MPL-2.0
//! Captured full-source transfers. Preparation performs bounded I/O; publication
//! holds every actor and uses a single durable recovery group marker.
use super::*;
use crate::power::captured::{CapturedRangeReader, StagingOptions};
use bareline_document::{
    history::EditMetadata,
    paged::{OwnedTextRange, PreparedSourceTransaction, SourceEdit, SourceTransactionPoll},
};
use std::ops::Range;
#[derive(Clone)]
pub struct PagedTransferCapture {
    pub source: PagedReadHandle,
    pub ranges: Vec<Range<TextOffset>>,
    pub destination: PagedReadHandle,
    pub at: TextOffset,
    pub copy: bool,
    pub source_selections: crate::power::SelectionSet,
    pub destination_selections: crate::power::SelectionSet,
}
struct Staged {
    capture: PagedTransferCapture,
    source: Option<PreparedSourceTransaction>,
    destination: PreparedSourceTransaction,
    foreign: Vec<(
        bareline_document::source::Generation,
        bareline_file_io::codecs::disk::DiskDecoded,
    )>,
}
#[derive(Clone)]
struct Published {
    snapshots: Vec<PagedSnapshot>,
    selections: Vec<crate::power::SelectionSet>,
    installed: std::collections::BTreeSet<u64>,
}

struct TransferCompletion<T> {
    sender: Option<SyncSender<Result<T, String>>>,
    wakes: Vec<JobWake>,
}

impl<T> TransferCompletion<T> {
    fn new(sender: SyncSender<Result<T, String>>, endpoints: &[Endpoint]) -> Self {
        Self {
            sender: Some(sender),
            wakes: endpoints
                .iter()
                .map(|endpoint| JobWake::new(endpoint.notify.clone()))
                .collect(),
        }
    }

    fn complete(mut self, result: Result<T, String>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(result);
        }
        for wake in self.wakes.drain(..) {
            wake.finish();
        }
    }
}

impl<T> Drop for TransferCompletion<T> {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(Err("Transfer worker failed".into()));
        }
    }
}

struct HistoryCompletion {
    output: HistoryResult,
    wakes: Vec<JobWake>,
    finished: bool,
}

impl HistoryCompletion {
    fn new(output: HistoryResult, endpoints: &[Endpoint]) -> Self {
        Self {
            output,
            wakes: endpoints
                .iter()
                .map(|endpoint| JobWake::new(endpoint.notify.clone()))
                .collect(),
            finished: false,
        }
    }

    fn complete(mut self, result: Result<Published, String>) {
        *self.output.lock().unwrap_or_else(|error| error.into_inner()) = Some(result);
        self.finished = true;
        for wake in self.wakes.drain(..) {
            wake.finish();
        }
    }
}

impl Drop for HistoryCompletion {
    fn drop(&mut self) {
        if !self.finished {
            *self.output.lock().unwrap_or_else(|error| error.into_inner()) =
                Some(Err("Transfer history worker failed".into()));
        }
    }
}

enum Phase {
    Preparing(Receiver<Result<Staged, String>>),
    Committing(Receiver<Result<Published, String>>),
    Installing(Published),
    Finished,
}
pub struct PagedTransfer {
    phase: Phase,
    options: StagingOptions,
}
impl PagedTransfer {
    pub fn start(capture: PagedTransferCapture, options: StagingOptions) -> Result<Self, String> {
        if capture.ranges.is_empty() || capture.ranges.len() > 4096 {
            return Err("Invalid transfer selection count".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let config = options.clone();
        worker()
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    let _ = tx.send(stage(capture, &config));
                }),
            )
            .map_err(|_| "Transfer worker queue is full")?;
        Ok(Self {
            phase: Phase::Preparing(rx),
            options,
        })
    }
    pub fn cancel(&self) {
        self.options.cancellation.cancel();
    }
    pub fn pump(&mut self, views: &mut [&mut PagedEditorSurface]) -> Option<Result<(), String>> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        match phase {
            Phase::Preparing(rx) => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.phase = Phase::Preparing(rx);
                    None
                }
                Err(_) => Some(Err("Transfer preparation stopped".into())),
                Ok(Err(error)) => Some(Err(error)),
                Ok(Ok(staged)) => {
                    if self.options.cancellation.check().is_err() {
                        return Some(Err("Transfer cancelled".into()));
                    }
                    let mut endpoints = Vec::new();
                    let captured_handles = [staged.capture.source.clone(), staged.capture.destination.clone()];
                    for captured in &captured_handles {
                        if endpoints
                            .iter()
                            .any(|endpoint: &Endpoint| endpoint.snapshot.same_document(captured.snapshot()))
                        {
                            continue;
                        }
                        let Some(view) = views
                            .iter_mut()
                            .find(|view| view.snapshot.same_document(captured.snapshot()))
                        else {
                            return Some(Err("Transfer document closed".into()));
                        };
                        if view.busy() {
                            self.phase = Phase::Preparing(ready(staged));
                            return None;
                        }
                        if (view.surface.user_read_only || view.following || view.captured.is_some())
                            && (!staged.capture.copy
                                || captured.snapshot().same_document(staged.capture.destination.snapshot()))
                            || view.snapshot.identity_token() != captured.snapshot().identity_token()
                        {
                            return Some(Err("Transfer document changed or became read-only".into()));
                        }
                        endpoints.push(Endpoint::capture(view));
                    }
                    endpoints.sort_by_key(|endpoint| endpoint.snapshot.identity_token().0);
                    let config = self.options.clone();
                    let (tx, rx) = mpsc::sync_channel(1);
                    if worker()
                        .submit(
                            WorkKind::Bulk,
                            Box::new(move || {
                                let completion = TransferCompletion::new(tx, &endpoints);
                                let result = commit(staged, &endpoints, &config);
                                completion.complete(result);
                            }),
                        )
                        .is_err()
                    {
                        return Some(Err("Transfer worker queue is full".into()));
                    }
                    self.phase = Phase::Committing(rx);
                    None
                }
            },
            Phase::Committing(rx) => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.phase = Phase::Committing(rx);
                    None
                }
                Err(_) => Some(Err("Transfer worker stopped".into())),
                Ok(Err(error)) => Some(Err(error)),
                Ok(Ok(published)) => {
                    self.phase = Phase::Installing(published);
                    self.pump(views)
                }
            },
            Phase::Installing(mut published) => {
                for (target, selections) in published.snapshots.iter().zip(&published.selections) {
                    let mut found = false;
                    let mut installed = true;
                    for view in views.iter_mut().filter(|view| view.snapshot.same_document(target)) {
                        found = true;
                        view.refresh_peer();
                        view.pump();
                        if view.snapshot.revision.0 > target.revision.0 {
                            return Some(Err(
                                "Transfer committed; view changed before selection installation".into()
                            ));
                        }
                        if view.snapshot.revision.0 < target.revision.0 || view.busy() {
                            if !view.busy()
                                && let Some(error) = &view.error
                            {
                                return Some(Err(format!(
                                    "Transfer committed; viewport installation failed: {error}"
                                )));
                            }
                            installed = false;
                        } else if !published.installed.contains(&target.identity_token().0) {
                            install_selection(view, selections.clone());
                        }
                    }
                    if !found {
                        return Some(Err(
                            "Transfer committed; document view closed before installation".into()
                        ));
                    }
                    if installed {
                        published.installed.insert(target.identity_token().0);
                    }
                }
                if published.installed.len() == published.snapshots.len() {
                    Some(Ok(()))
                } else {
                    self.phase = Phase::Installing(published);
                    None
                }
            }
            Phase::Finished => None,
        }
    }
}
impl Drop for PagedTransfer {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn ready(value: Staged) -> Receiver<Result<Staged, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    let _ = tx.send(Ok(value));
    rx
}
#[derive(Clone)]
struct Endpoint {
    views: std::sync::Weak<()>,
    actor: PagedSession,
    snapshot: PagedSnapshot,
    peer: Arc<Mutex<PeerState>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl Endpoint {
    fn capture(view: &PagedEditorSurface) -> Self {
        Self {
            views: Arc::downgrade(&view.views),
            actor: view.actor.clone(),
            snapshot: view.snapshot.clone(),
            peer: view.peer.clone(),
            notify: view.notify.clone(),
        }
    }
}
fn commit(mut staged: Staged, endpoints: &[Endpoint], options: &StagingOptions) -> Result<Published, String> {
    options.cancellation.check().map_err(|_| "Transfer cancelled")?;
    let mut guards = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        guards.push(endpoint.actor.lock_document().map_err(|error| error.to_string())?);
    }
    for (opened, endpoint) in guards.iter().zip(endpoints) {
        if opened.document().snapshot().identity_token() != endpoint.snapshot.identity_token() {
            return Err("Transfer document changed".into());
        }
    }
    let destination = endpoints
        .iter()
        .position(|endpoint| endpoint.snapshot.same_document(staged.capture.destination.snapshot()))
        .ok_or("Missing transfer destination")?;
    for (generation, store) in &staged.foreign {
        guards[destination]
            .retain_foreign_source(*generation, store)
            .map_err(|error| error.to_string())?;
    }
    let mut tokens = Vec::new();
    let mut modified = Vec::new();
    let mut destination_token = Some(staged.destination);
    for (index, endpoint) in endpoints.iter().enumerate() {
        let token = if index == destination {
            destination_token.take()
        } else {
            staged.source.take()
        };
        if let Some(token) = token {
            if !endpoint.actor.recovery_enabled() {
                return Err("Enable recovery before transferring text".into());
            }
            endpoint
                .actor
                .ensure_recovery(&guards[index], &endpoint.snapshot, endpoint.notify.clone())
                .map_err(|error| error.to_string())?;
            tokens.push(token);
            modified.push(index);
        }
    }
    let mut documents: Vec<_> = guards
        .iter_mut()
        .enumerate()
        .filter(|(index, _)| modified.contains(index))
        .map(|(_, opened)| opened.document_mut())
        .collect();
    options.cancellation.check().map_err(|_| "Transfer cancelled")?;
    let snapshots;
    let selections;
    if documents.len() == 1 {
        let lease = documents[0]
            .lease_source_transaction(tokens.remove(0))
            .map_err(|e| format!("{e:?}"))?;
        endpoints[modified[0]]
            .actor
            .append_recovery_sources(lease.snapshot(), lease.edits(), options.quota)
            .map_err(|error| error.to_string())?;
        snapshots = vec![lease.snapshot().clone()];
        selections = vec![selection_set(&lease.metadata().after, 0)];
        lease.publish();
    } else {
        let lease = bareline_document::paged_group::lease_source_group(&mut documents, tokens, &options.budget)
            .map_err(|e| format!("{e:?}"))?;
        snapshots = lease
            .members()
            .iter()
            .map(|member| member.snapshot().clone())
            .collect::<Vec<_>>();
        selections = lease
            .members()
            .iter()
            .map(|member| selection_set(&member.metadata().after, 0))
            .collect::<Vec<_>>();
        let group_id = lease.id();
        register_group(group_id, endpoints, options, &staged.capture, &selections)?;
        let edits = lease
            .members()
            .iter()
            .map(|member| bareline_file_io::paged_recovery::group::GroupEdits::Source(member.edits()))
            .collect::<Vec<_>>();
        let sessions = modified
            .iter()
            .map(|index| endpoints[*index].actor.clone())
            .collect::<Vec<_>>();
        if let Err(error) = bareline_file_io::paged_service::commit_recovery_group(
            &sessions,
            &snapshots,
            &edits,
            publication_id(),
            options.quota,
            &options.cancellation,
        ) {
            unregister_group(group_id);
            return Err(error.to_string());
        }
        lease.publish();
    }
    for index in modified {
        if let Ok(mut peer) = endpoints[index].peer.lock() {
            peer.epoch = peer.epoch.wrapping_add(1);
        }
    }
    Ok(Published {
        snapshots,
        selections,
        installed: Default::default(),
    })
}
fn stage(mut capture: PagedTransferCapture, options: &StagingOptions) -> Result<Staged, String> {
    use std::io::Read;
    let same = capture.source.snapshot().same_document(capture.destination.snapshot());
    capture.ranges.sort_by_key(|range| range.start);
    if capture
        .ranges
        .iter()
        .any(|range| range.start >= range.end || range.end.0 > capture.source.snapshot().len())
        || capture.ranges.windows(2).any(|ranges| ranges[0].end > ranges[1].start)
        || capture.at.0 > capture.destination.snapshot().len()
    {
        return Err("Invalid transfer ranges".into());
    }
    if same
        && !capture.copy
        && capture
            .ranges
            .iter()
            .any(|range| range.start <= capture.at && capture.at <= range.end)
    {
        return Err("Drop is inside the moved selection".into());
    }
    for range in &capture.ranges {
        for offset in [range.start.0, range.end.0] {
            if crate::paged_navigation::snap_grapheme(&capture.source, offset, &options.budget, &options.cancellation)?
                != offset
            {
                return Err("Transfer selection splits a grapheme".into());
            }
        }
    }
    if crate::paged_navigation::snap_grapheme(
        &capture.destination,
        capture.at.0,
        &options.budget,
        &options.cancellation,
    )? != capture.at.0
    {
        return Err("Transfer drop splits a grapheme".into());
    }
    let mut builder = bareline_file_io::owned_store::StreamingStoreBuilder::new(
        &options.cache,
        options.quota,
        options.platform.clone(),
        options.source_options,
        options.budget.clone(),
        options.cancellation.clone(),
    )
    .map_err(|e| e.to_string())?;
    let mut spans = Vec::new();
    for range in &capture.ranges {
        let start = builder.len();
        let mut reader = CapturedRangeReader::new(
            capture.source.clone(),
            range.clone(),
            options.budget.clone(),
            options.cancellation.clone(),
        )
        .map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65536];
        loop {
            let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            std::io::Write::write_all(&mut builder, &buffer[..count]).map_err(|e| e.to_string())?;
        }
        spans.push(start..builder.len());
    }
    let length = builder.len();
    let source = builder.finish().map_err(|e| e.to_string())?;
    let empty = OwnedTextRange {
        source: source.clone(),
        range: 0..0,
    };
    let insertion = SourceEdit {
        range: capture.at..capture.at,
        inverse: empty.clone(),
        inserted: OwnedTextRange {
            source: source.clone(),
            range: 0..length,
        },
    };
    let deletes: Vec<_> = capture
        .ranges
        .iter()
        .zip(spans)
        .map(|(range, span)| SourceEdit {
            range: range.clone(),
            inverse: OwnedTextRange {
                source: source.clone(),
                range: span,
            },
            inserted: empty.clone(),
        })
        .collect();
    let mut destination_edits = vec![insertion];
    if same && !capture.copy {
        destination_edits.extend(deletes.iter().cloned());
    }
    destination_edits.sort_by_key(|edit| edit.range.start);
    let insertion_index = destination_edits
        .iter()
        .position(|edit| edit.range.is_empty())
        .ok_or("Missing insertion")?;
    let mut metadata = EditMetadata::default();
    metadata.before = history_selection(if same {
        &capture.source_selections
    } else {
        &capture.destination_selections
    });
    let before_removed = if same && !capture.copy {
        capture
            .ranges
            .iter()
            .filter(|range| range.end < capture.at)
            .map(|range| range.end.0 - range.start.0)
            .sum()
    } else {
        0
    };
    let caret = TextOffset(capture.at.0 - before_removed + usize::try_from(length).map_err(|_| "Transfer too large")?);
    metadata.after = vec![bareline_document::history::Selection { anchor: caret, caret }];
    let request = capture
        .destination
        .snapshot()
        .prepare_source_transaction(destination_edits, metadata, options.budget.clone())
        .map_err(|e| format!("{e:?}"))?
        .with_inserted_provenance_parts(
            insertion_index,
            capture.source.snapshot().clone(),
            capture.ranges.clone(),
        )
        .map_err(|e| format!("{e:?}"))?;
    let destination = validate(request, &capture, options)?;
    let source_token = if !same && !capture.copy {
        Some(validate(
            capture
                .source
                .snapshot()
                .prepare_source_transaction(
                    deletes,
                    EditMetadata {
                        before: history_selection(&capture.source_selections),
                        after: vec![bareline_document::history::Selection {
                            anchor: capture.ranges[0].start,
                            caret: capture.ranges[0].start,
                        }],
                        ..Default::default()
                    },
                    options.budget.clone(),
                )
                .map_err(|e| format!("{e:?}"))?,
            &capture,
            options,
        )?)
    } else {
        None
    };
    let mut foreign = Vec::new();
    if !same {
        let base = capture.source.original_store()?;
        for piece in capture.source.snapshot().pieces() {
            use bareline_document::paged::PagedPiece;
            let original = match piece {
                PagedPiece::Original { source, .. } | PagedPiece::OriginalOwned { source, .. } => Some(source),
                PagedPiece::OwnedSource {
                    original: Some((source, _)),
                    ..
                } => Some(source),
                _ => None,
            };
            if let Some(source) = original {
                if foreign.iter().any(|(generation, _)| *generation == source.generation()) {
                    continue;
                }
                let store = base
                    .foreign_source(source.generation())
                    .map_err(|e| format!("{e:?}"))?
                    .unwrap_or_else(|| base.clone());
                if !source.has_owned_loader() {
                    store
                        .attach_text_loader(source, &options.cancellation)
                        .map_err(|e| format!("{e:?}"))?;
                }
                foreign.push((source.generation(), store));
            }
        }
    }
    Ok(Staged {
        capture,
        source: source_token,
        destination,
        foreign,
    })
}
fn validate(
    mut request: bareline_document::paged::SourceTransactionRequest,
    capture: &PagedTransferCapture,
    options: &StagingOptions,
) -> Result<PreparedSourceTransaction, String> {
    loop {
        options.cancellation.check().map_err(|_| "Transfer cancelled")?;
        match request.poll() {
            SourceTransactionPoll::Ready(token) => return Ok(token),
            SourceTransactionPoll::Progress => {}
            SourceTransactionPoll::Pending(ticket) => {
                if !request.resolve_owned(ticket).map_err(|e| format!("{e:?}"))? {
                    let source = request
                        .pending_snapshot()
                        .is_some_and(|snapshot| snapshot.same_document(capture.source.snapshot()));
                    let handled = if source {
                        capture
                            .source
                            .resolve_captured_page(ticket)
                            .map_err(|error| error.to_string())
                    } else {
                        capture
                            .destination
                            .resolve_captured_page(ticket)
                            .map_err(|error| error.to_string())
                    }?;
                    if !handled {
                        std::thread::yield_now();
                    }
                }
            }
            SourceTransactionPoll::Failed(error) => return Err(format!("{error:?}")),
            SourceTransactionPoll::Unavailable(reason) => return Err(format!("Source unavailable: {reason:?}")),
            SourceTransactionPoll::Cancelled => return Err("Transfer cancelled".into()),
            SourceTransactionPoll::Finished => return Err("Transfer request finished".into()),
        }
    }
}

#[derive(Clone)]
struct GroupRecord {
    id: bareline_document::group::UndoGroup,
    endpoints: Vec<Endpoint>,
    options: StagingOptions,
    before: Vec<crate::power::SelectionSet>,
    after: Vec<crate::power::SelectionSet>,
}
type HistoryResult = Arc<Mutex<Option<Result<Published, String>>>>;
#[derive(Default)]
struct Registry {
    groups: Vec<GroupRecord>,
    pending: std::collections::BTreeMap<u64, HistoryResult>,
}
fn registry() -> &'static Mutex<Registry> {
    static VALUE: OnceLock<Mutex<Registry>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(Registry::default()))
}
fn publication_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64)
        .wrapping_add(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}
fn register_group(
    id: bareline_document::group::UndoGroup,
    endpoints: &[Endpoint],
    options: &StagingOptions,
    capture: &PagedTransferCapture,
    after: &[crate::power::SelectionSet],
) -> Result<(), String> {
    let mut registry = registry().lock().map_err(|_| "Transfer group registry stopped")?;
    registry
        .groups
        .retain(|group| group.endpoints.iter().any(|endpoint| endpoint.views.strong_count() > 0));
    if registry.groups.len() >= 100 {
        return Err("Linked transfer history limit reached".into());
    }
    registry
        .groups
        .try_reserve(1)
        .map_err(|_| "Transfer group memory limit")?;
    let before = endpoints
        .iter()
        .map(|endpoint| {
            if endpoint.snapshot.same_document(capture.source.snapshot()) {
                capture.source_selections.clone()
            } else {
                capture.destination_selections.clone()
            }
        })
        .collect();
    registry.groups.push(GroupRecord {
        id,
        endpoints: endpoints.to_vec(),
        options: options.clone(),
        before,
        after: after.to_vec(),
    });
    Ok(())
}
fn unregister_group(id: bareline_document::group::UndoGroup) {
    if let Ok(mut registry) = registry().lock() {
        registry.groups.retain(|group| group.id != id);
    }
}
/// Public readiness includes the terminal receipt until this participant's
/// authoritative viewport and selection have actually been installed.
pub(super) fn history_pending(view: &PagedEditorSurface) -> bool {
    registry().lock().map_or(true, |registry| {
        registry.pending.contains_key(&view.snapshot.identity_token().0)
    })
}
pub(super) fn history_busy(view: &PagedEditorSurface) -> bool {
    registry()
        .lock()
        .ok()
        .and_then(|registry| registry.pending.get(&view.snapshot.identity_token().0).cloned())
        .is_some_and(|result| result.lock().map_or(true, |result| result.is_none()))
}
pub(super) fn pump_history(view: &mut PagedEditorSurface) -> bool {
    let id = view.snapshot.identity_token().0;
    let result = registry()
        .lock()
        .ok()
        .and_then(|registry| registry.pending.get(&id).cloned());
    let Some(result) = result.and_then(|result| result.lock().ok().and_then(|result| result.clone())) else {
        return false;
    };
    let done = match result {
        Err(error) => {
            view.error = Some(error);
            true
        }
        Ok(published) => {
            let Some(index) = published
                .snapshots
                .iter()
                .position(|snapshot| snapshot.same_document(&view.snapshot))
            else {
                return false;
            };
            let target = &published.snapshots[index];
            if view.snapshot.revision.0 < target.revision.0 {
                view.refresh_peer();
                if !view.power_actor_busy() && view.error.is_some() {
                    true
                } else {
                    false
                }
            } else if view.power_preparing || !view.power_inputs.is_empty() || view.power_actor_busy() {
                false
            } else if view.snapshot.revision == target.revision {
                install_selection(view, published.selections[index].clone());
                true
            } else {
                view.error = Some("Linked history committed; view changed before selection installation".into());
                true
            }
        }
    };
    if done && let Ok(mut registry) = registry().lock() {
        registry.pending.remove(&id);
    }
    done
}
pub(super) fn try_history(view: &mut PagedEditorSurface, undo: bool) -> Option<Result<(), String>> {
    let group = {
        let opened = match view.actor.try_document() {
            Ok(opened) => opened,
            Err(_) => return Some(Err("Transfer actor is busy".into())),
        };
        opened.document().history_group(undo)
    }?;
    let record = match registry().lock() {
        Ok(registry) => registry.groups.iter().find(|record| record.id == group).cloned(),
        Err(_) => None,
    };
    let Some(record) = record else {
        return Some(Err("Linked transfer members are unavailable".into()));
    };
    let state: HistoryResult = Arc::new(Mutex::new(None));
    {
        let mut registry = match registry().lock() {
            Ok(registry) => registry,
            Err(_) => return Some(Err("Transfer group registry stopped".into())),
        };
        for endpoint in &record.endpoints {
            let id = endpoint.snapshot.identity_token().0;
            if endpoint.views.strong_count() == 0
                && registry
                    .pending
                    .get(&id)
                    .is_some_and(|state| state.lock().is_ok_and(|result| result.is_some()))
            {
                registry.pending.remove(&id);
            }
        }
        if record
            .endpoints
            .iter()
            .any(|endpoint| registry.pending.contains_key(&endpoint.snapshot.identity_token().0))
        {
            return Some(Err("Linked history operation pending".into()));
        }
        for endpoint in &record.endpoints {
            registry
                .pending
                .insert(endpoint.snapshot.identity_token().0, state.clone());
        }
    }
    let output = state.clone();
    let rejected_notifications = record
        .endpoints
        .iter()
        .map(|endpoint| endpoint.notify.clone())
        .collect::<Vec<_>>();
    let queued = worker().submit(
        WorkKind::General,
        Box::new(move || {
            let completion = HistoryCompletion::new(output, &record.endpoints);
            let result = commit_history(&record, undo);
            completion.complete(result);
        }),
    );
    if queued.is_err() {
        if let Ok(mut state) = state.lock() {
            *state = Some(Err("Transfer history queue is full".into()));
        }
        for notify in rejected_notifications {
            JobWake::new(notify).finish();
        }
        return Some(Err("Transfer history queue is full".into()));
    }
    Some(Ok(()))
}
fn commit_history(record: &GroupRecord, undo: bool) -> Result<Published, String> {
    let mut guards = Vec::new();
    for endpoint in &record.endpoints {
        guards.push(endpoint.actor.lock_document().map_err(|error| error.to_string())?);
    }
    let mut documents: Vec<_> = guards.iter_mut().map(|opened| opened.document_mut()).collect();
    let lease =
        bareline_document::paged_group::lease_history_group(&mut documents, record.id, undo, &record.options.budget)
            .map_err(|error| format!("{error:?}"))?;
    let snapshots = lease
        .members()
        .iter()
        .map(|member| member.snapshot().clone())
        .collect::<Vec<_>>();
    let edits = lease
        .members()
        .iter()
        .map(|member| bareline_file_io::paged_recovery::group::GroupEdits::History(member.edits()))
        .collect::<Vec<_>>();
    let sessions = record
        .endpoints
        .iter()
        .map(|endpoint| endpoint.actor.clone())
        .collect::<Vec<_>>();
    bareline_file_io::paged_service::commit_recovery_group(
        &sessions,
        &snapshots,
        &edits,
        publication_id(),
        record.options.quota,
        &Cancellation::default(),
    )
    .map_err(|error| error.to_string())?;
    lease.publish();
    for endpoint in &record.endpoints {
        if let Ok(mut peer) = endpoint.peer.lock() {
            peer.epoch = peer.epoch.wrapping_add(1);
        }
    }
    Ok(Published {
        snapshots,
        selections: if undo {
            record.before.clone()
        } else {
            record.after.clone()
        },
        installed: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, io,
        path::Path,
        time::{Duration, Instant},
    };
    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            fs::File::open(path)
        }
        fn identity(&self, file: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            if target.exists() {
                fs::remove_file(target)?;
            }
            fs::rename(staged, target)
        }
    }
    fn drain(views: &mut [&mut PagedEditorSurface]) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            for view in views.iter_mut() {
                view.pump();
            }
            if views.iter().all(|view| !view.busy()) {
                break;
            }
            assert!(Instant::now() < until, "transfer timeout");
            std::thread::yield_now();
        }
    }
    fn open(path: PathBuf, root: &Path, budget: &Budget) -> PagedEditorSurface {
        open_with(path, root, budget, Arc::new(Platform))
    }
    fn open_with(
        path: PathBuf,
        root: &Path,
        budget: &Budget,
        platform: Arc<dyn LocalFileSystem>,
    ) -> PagedEditorSurface {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
        };
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(16 << 20),
                cache: root.into(),
                options: DiskOptions {
                    temp_quota_bytes: 64 << 20,
                    interpret: None,
                },
                source_options: Default::default(),
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open")
        };
        let mut view = PagedEditorSurface::new(opened, budget.clone(), Arc::new(|| {})).unwrap();
        view.enable_recovery(root.join("recovery"), platform);
        drain(&mut [&mut view]);
        view.surface.user_read_only = false;
        view
    }
    struct StarveAfterMarker(std::sync::atomic::AtomicBool);
    impl LocalFileSystem for StarveAfterMarker {
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            Platform.validate_target(path)
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(if self.0.load(std::sync::atomic::Ordering::Acquire) {
                0
            } else {
                u64::MAX
            })
        }
        fn guard_directory(&self, path: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Platform.guard_directory(path)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            Platform.open_sealed_read(path)
        }
        fn identity(&self, file: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            Platform.identity(file)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            Platform.commit(staged, target, existed)?;
            if target
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".commit.json"))
            {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        }
    }
    #[test]
    fn committed_group_with_failed_journal_admission_blocks_old_writer() {
        let root = std::env::temp_dir().join(format!(
            "bareline-group-admission-{}-{}",
            std::process::id(),
            publication_id()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        fs::write(&a, b"source").unwrap();
        fs::write(&b, b"target").unwrap();
        let budget = Budget::new(64 << 20);
        let platform = Arc::new(StarveAfterMarker(std::sync::atomic::AtomicBool::new(false)));
        let mut source = open_with(a, &root, &budget, platform.clone());
        let mut destination = open_with(b, &root, &budget, platform.clone());
        let options = StagingOptions {
            cache: root.clone(),
            quota: 64 << 20,
            platform: platform.clone(),
            source_options: Default::default(),
            budget: budget.clone(),
            memory: 1 << 20,
            cancellation: Cancellation::default(),
        };
        let capture = PagedTransferCapture {
            source: source.read_handle(),
            ranges: vec![TextOffset(0)..TextOffset(6)],
            destination: destination.read_handle(),
            at: TextOffset(6),
            copy: false,
            source_selections: Selection { anchor: 0, caret: 6 }.into(),
            destination_selections: Selection { anchor: 6, caret: 6 }.into(),
        };
        let mut transfer = PagedTransfer::start(capture, options).unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(result) = transfer.pump(&mut [&mut source, &mut destination]) {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(destination.snapshot.len(), 12);
        assert!(destination.recovery_status().error.is_some());
        platform.0.store(false, std::sync::atomic::Ordering::Release);
        let mut next = destination.snapshot.clone();
        next.revision = bareline_document::Revision(next.revision.0 + 1);
        assert!(
            destination.actor.append_recovery_edits(&next, &[]).is_err(),
            "old journal must remain poisoned after space returns"
        );
        let restored = bareline_file_io::paged_recovery::restore(
            &destination.recovery_status().directory.unwrap(),
            platform,
            budget,
            Budget::new(16 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(restored.transcoded.document.snapshot().len(), 12);
        let group = source
            .actor
            .lock_document()
            .unwrap()
            .document()
            .history_group(true)
            .unwrap();
        unregister_group(group);
        drop(transfer);
        drop(restored);
        drop(source);
        drop(destination);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn opaque_cross_document_move_restart_and_linked_undo_preserve_raw_bytes() {
        let root = std::env::temp_dir().join(format!(
            "bareline-atomic-transfer-{}-{}",
            std::process::id(),
            publication_id()
        ));
        fs::create_dir(&root).unwrap();
        let source_path = root.join("source.txt");
        let destination_path = root.join("destination.txt");
        fs::write(&source_path, [0xff, 0xfe, 0x00, 0xd8]).unwrap();
        fs::write(&destination_path, [0xff, 0xfe, 0x41, 0x00]).unwrap();
        let budget = Budget::new(64 << 20);
        let mut source = open(source_path, &root, &budget);
        let mut destination = open(destination_path, &root, &budget);
        let options = StagingOptions {
            cache: root.clone(),
            quota: 64 << 20,
            platform: Arc::new(Platform),
            source_options: Default::default(),
            budget: budget.clone(),
            memory: 1 << 20,
            cancellation: Cancellation::default(),
        };
        let mut transfer = PagedTransfer::start(
            PagedTransferCapture {
                source: source.read_handle(),
                ranges: vec![TextOffset(0)..TextOffset(3)],
                destination: destination.read_handle(),
                at: TextOffset(1),
                copy: false,
                source_selections: Selection { anchor: 0, caret: 3 }.into(),
                destination_selections: Selection { anchor: 1, caret: 1 }.into(),
            },
            options,
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(result) = transfer.pump(&mut [&mut source, &mut destination]) {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(source.snapshot.len(), 0);
        assert_eq!(destination.snapshot.len(), 4);
        let source_recovery = source.recovery_status().directory.unwrap();
        let destination_recovery = destination.recovery_status().directory.unwrap();
        let restored = bareline_file_io::paged_recovery::restore(
            &destination_recovery,
            Arc::new(Platform),
            budget.clone(),
            Budget::new(16 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        let mut raw = Vec::new();
        restored
            .transcoded
            .store
            .write_snapshot(
                &restored.transcoded.document.snapshot(),
                restored.transcoded.source.source().generation(),
                bareline_file_io::codecs::Encoding::Utf16Le,
                true,
                &mut raw,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(raw, [0xff, 0xfe, 0x41, 0x00, 0x00, 0xd8]);
        let restored_source = bareline_file_io::paged_recovery::restore(
            &source_recovery,
            Arc::new(Platform),
            budget.clone(),
            Budget::new(16 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(restored_source.transcoded.document.snapshot().len(), 0);
        // An ordinary revision after the group remains authoritative even when
        // its latest-root pointer cannot be replaced.
        let pointer = destination_recovery.join("paged-root.json");
        let old_pointer = fs::read(&pointer).unwrap();
        fs::remove_file(&pointer).unwrap();
        fs::create_dir(&pointer).unwrap();
        destination.enqueue(Input::Insert("Z".into()));
        drain(&mut [&mut source, &mut destination]);
        assert_eq!(destination.snapshot.len(), 5);
        let later = bareline_file_io::paged_recovery::restore(
            &destination_recovery,
            Arc::new(Platform),
            budget.clone(),
            Budget::new(16 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(later.transcoded.document.snapshot().len(), 5);
        fs::remove_dir(&pointer).unwrap();
        fs::write(&pointer, old_pointer).unwrap();
        destination.error = None;
        destination.enqueue(Input::Undo);
        drain(&mut [&mut source, &mut destination]);
        assert_eq!(destination.snapshot.len(), 4);
        let group = source
            .actor
            .lock_document()
            .unwrap()
            .document()
            .history_group(true)
            .unwrap();
        source.enqueue(Input::Undo);
        drain(&mut [&mut source, &mut destination]);
        assert_eq!(source.snapshot.len(), 3);
        assert_eq!(destination.snapshot.len(), 1);
        assert_eq!(
            source.global_selection_set().selections,
            vec![Selection { anchor: 0, caret: 3 }]
        );
        assert_eq!(source.global_selection_set().primary, 0);
        assert_eq!(
            destination.global_selection_set().selections,
            vec![Selection { anchor: 1, caret: 1 }]
        );
        source.enqueue(Input::Redo);
        drain(&mut [&mut source, &mut destination]);
        assert_eq!(source.snapshot.len(), 0);
        assert_eq!(destination.snapshot.len(), 4);
        drop(destination);
        source.enqueue(Input::Undo);
        drain(&mut [&mut source]);
        assert_eq!(source.snapshot.len(), 3);
        source.enqueue(Input::Redo);
        drain(&mut [&mut source]);
        assert_eq!(
            source.snapshot.len(),
            0,
            "closed participant terminal must not block the next linked history operation"
        );
        unregister_group(group);
        drop(transfer);
        drop(restored);
        drop(restored_source);
        drop(later);
        drop(source);
        let _ = fs::remove_dir_all(root);
    }
}

fn history_selection(set: &crate::power::SelectionSet) -> Vec<bareline_document::history::Selection> {
    set.selections
        .iter()
        .map(|selection| bareline_document::history::Selection {
            anchor: TextOffset(selection.anchor),
            caret: TextOffset(selection.caret),
        })
        .collect()
}
fn selection_set(selections: &[bareline_document::history::Selection], primary: usize) -> crate::power::SelectionSet {
    crate::power::SelectionSet {
        selections: selections
            .iter()
            .map(|selection| Selection {
                anchor: selection.anchor.0,
                caret: selection.caret.0,
            })
            .collect(),
        primary,
    }
}
fn install_selection(view: &mut PagedEditorSurface, set: crate::power::SelectionSet) {
    view.global_selections = set;
    view.selection_token = view.selection_token.wrapping_add(1);
    view.project_global_selection();
}
