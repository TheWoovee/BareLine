// SPDX-License-Identifier: MPL-2.0
//! Paged save orchestration owned by `file-io`.
//!
//! The paged editor surface previously drove the low-level `lifecycle` save
//! primitives (`PagedSavePolicy`, `save_paged_cancellable`, `guard_copy_source`)
//! and the `FileError` mapping itself, duplicating responsibilities that belong
//! with the rest of the paged-document file lifecycle. That policy now lives
//! here: the surface hands over the pieces it holds and gets back the new
//! fingerprint, so save/fingerprint decisions sit with `file-io`.
//!
//! The caller must release the paged document guard before dispatching [`PagedLifecycleCommand::Save`] — the
//! full-file write is long-running and readers keep serving from the released
//! actor — and re-acquire it afterwards to publish the returned fingerprint.
use crate::cancellation::Cancellation;
use crate::codecs::disk::DiskDecoded;
use crate::codecs::failure::EncodingFailure;
use crate::lifecycle::{
    FileError, Fingerprint, PagedOpened, PagedSavePolicy, PreparedDestination, SaveCleanup, SaveConflict,
    guard_copy_source, save_paged_to_cancellable,
};
use bareline_document::source::Generation;
use bareline_document::{ContentStateId, Revision, paged::PagedSnapshot};
use bareline_platform::LocalFileSystem;
use std::path::PathBuf;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, TryLockError,
    atomic::{AtomicU64, Ordering},
};

/// Exact fence attached to every lifecycle command and terminal receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagedLifecycleStamp {
    pub document: u64,
    pub revision: Revision,
    pub content: ContentStateId,
    pub source_generation: Generation,
    pub operation_generation: u64,
}

#[derive(Clone, Debug)]
pub struct PagedLifecycleState {
    pub stamp: PagedLifecycleStamp,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub saved_state: Option<ContentStateId>,
    pub save_as_required: bool,
    pub encoding: crate::codecs::state::EncodingState,
    pub recovery_origin: Option<PathBuf>,
}

pub enum PagedLifecycleCommand {
    Save {
        requested: PagedLifecycleStamp,
        destination: PreparedDestination,
        copy_only: bool,
        platform: Arc<dyn LocalFileSystem>,
    },
    RetryRecovery {
        requested: PagedLifecycleStamp,
    },
    RetireRecovery {
        requested: PagedLifecycleStamp,
    },
    Spill {
        requested: PagedLifecycleStamp,
        prepared: bareline_document::spill::PreparedSpill,
    },
}

#[derive(Debug)]
pub enum PagedLifecycleError {
    Busy,
    Changed,
    Cancelled,
    Encoding(EncodingFailure),
    Conflict(SaveConflict),
    SourceUnavailable(String),
    CleanupPending(SaveCleanup),
    Failed(FileError),
}

impl std::fmt::Display for PagedLifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Paged source is busy"),
            Self::Changed => f.write_str("Paged source changed; retry the operation"),
            Self::Cancelled => f.write_str("Paged operation cancelled"),
            Self::Encoding(error) => write!(f, "Encoding failed: {error:?}"),
            Self::Conflict(error) => write!(f, "Save conflict: {error:?}"),
            Self::SourceUnavailable(error) => write!(f, "Paged source unavailable: {error}"),
            Self::CleanupPending(error) => write!(f, "Saved document cleanup is pending: {error:?}"),
            Self::Failed(error) => write!(f, "{error:?}"),
        }
    }
}

#[derive(Debug)]
pub enum PagedTerminalOutcome {
    Saved { fingerprint: Fingerprint, copy_only: bool },
    RecoveryRetried { rebuild: bool },
    RecoveryRetired,
    Spilled,
}

#[derive(Debug)]
pub struct PagedLifecycleReceipt {
    pub requested: PagedLifecycleStamp,
    pub terminal: Result<PagedTerminalOutcome, PagedLifecycleError>,
}

impl PagedLifecycleReceipt {
    pub fn applies_to(&self, current: PagedLifecycleStamp) -> bool {
        self.requested == current
    }
}

struct PersistentState {
    path: PathBuf,
    fingerprint: Fingerprint,
    saved_state: Option<ContentStateId>,
    save_as_required: bool,
    recovery_origin: Option<PathBuf>,
    encoding_failure: Option<EncodingFailure>,
    save_conflicts: std::collections::VecDeque<SaveConflict>,
    save_cleanups: std::collections::VecDeque<SaveCleanup>,
    active_operation: Option<u64>,
}

struct OperationReservation {
    session: Arc<SessionInner>,
    generation: u64,
}

impl Drop for OperationReservation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.session.state.lock()
            && state.active_operation == Some(self.generation)
        {
            state.active_operation = None;
        }
    }
}

struct RetiredPagedGeneration {
    owner: Arc<()>,
    opened: Box<PagedOpened>,
    tail: Option<crate::tail::TailSession>,
}

#[derive(Default)]
struct PagedSpillState {
    pending: bool,
    config: Option<(PathBuf, Arc<dyn LocalFileSystem>, crate::source::SourceOptions)>,
    attempted: Option<(u64, ContentStateId)>,
    error: Option<String>,
}

pub struct PagedSpillReservation {
    pub cache: PathBuf,
    pub platform: Arc<dyn LocalFileSystem>,
    pub options: crate::source::SourceOptions,
}

struct SessionInner {
    actor: Mutex<Box<PagedOpened>>,
    state: Mutex<PersistentState>,
    operation_generation: AtomicU64,
    recovery_config: Mutex<Option<(PathBuf, Arc<dyn LocalFileSystem>)>>,
    recovery_platform: Arc<dyn LocalFileSystem>,
    recovery: Mutex<Option<crate::paged_recovery::PagedRecovery>>,
    recovery_discard: Mutex<Option<crate::recovery_retirement::DiscardTicket>>,
    recovery_cleanup_hold: Mutex<Option<crate::recovery_retirement::CleanupHold>>,
    recovery_status: Mutex<Arc<Mutex<crate::paged_recovery::PagedRecoveryStatus>>>,
    failed_retirements: Mutex<Vec<PathBuf>>,
    tail: Mutex<Option<crate::tail::TailSession>>,
    retired: Mutex<Vec<RetiredPagedGeneration>>,
    source_mismatch: std::sync::atomic::AtomicBool,
    generation_owner: Mutex<Arc<()>>,
    spill: Mutex<PagedSpillState>,
    gate_generation: Mutex<u64>,
    gate_released: Condvar,
}

/// The single mutable owner for a paged document's persistent identity and
/// document actor. Cloned views share this handle rather than copying state.
#[derive(Clone)]
pub struct PagedSession(Arc<SessionInner>);

/// Narrow document-only adapter. It deliberately has no path, fingerprint,
/// savepoint, encoding, recovery, or cleanup mutators.
pub struct PagedDocumentGuard<'a>(MutexGuard<'a, Box<PagedOpened>>);

pub struct PagedTailGuard<'a>(MutexGuard<'a, Option<crate::tail::TailSession>>);

impl PagedTailGuard<'_> {
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }
    pub fn start(
        &mut self,
        actor: &PagedDocumentGuard<'_>,
        platform: Arc<dyn LocalFileSystem>,
        budget: bareline_document::Budget,
        cancellation: Cancellation,
    ) -> Result<(), PagedLifecycleError> {
        if self.0.is_none() {
            *self.0 = Some(
                crate::tail::TailSession::new(&actor.0, platform, budget, cancellation).map_err(classify_file_error)?,
            );
        }
        Ok(())
    }
    pub fn read_page(&mut self, ticket: bareline_document::source::PageTicket) -> Result<bool, PagedLifecycleError> {
        match self.0.as_mut() {
            Some(tail) => tail.read_page(ticket).map_err(classify_file_error),
            None => Ok(false),
        }
    }
    pub fn append_receipt(&self) -> Option<crate::tail::AppendReceipt> {
        self.0.as_ref().and_then(crate::tail::TailSession::append_receipt)
    }
    pub fn pending(&self) -> bool {
        self.0.as_ref().is_some_and(crate::tail::TailSession::pending)
    }
    pub fn source_changed(&self) -> bool {
        self.0.as_ref().is_some_and(|tail| tail.source_changed)
    }
}

impl std::fmt::Debug for PagedDocumentGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PagedDocumentGuard")
    }
}

impl PagedDocumentGuard<'_> {
    pub fn document(&self) -> &bareline_document::paged::PagedDocument {
        &self.0.transcoded.document
    }
    pub fn document_mut(&mut self) -> &mut bareline_document::paged::PagedDocument {
        &mut self.0.transcoded.document
    }
    pub fn source_generation(&self) -> Generation {
        self.0.transcoded.source.source().generation()
    }
    pub fn retain_foreign_source(
        &self,
        generation: Generation,
        source: &DiskDecoded,
    ) -> Result<(), PagedLifecycleError> {
        self.0
            .transcoded
            .store
            .retain_foreign(generation, source)
            .map_err(|error| PagedLifecycleError::SourceUnavailable(format!("foreign source: {error:?}")))
    }
    pub fn read_source_page(
        &mut self,
        ticket: bareline_document::source::PageTicket,
    ) -> Result<(), PagedLifecycleError> {
        self.0.transcoded.source.read_page(ticket).map_err(|error| match error {
            FileError::Changed | FileError::Transcode(crate::codecs::disk::DiskError::Changed) => {
                PagedLifecycleError::Changed
            }
            other => PagedLifecycleError::SourceUnavailable(format!("{other:?}")),
        })
    }
}

impl PagedSession {
    fn reserve_operation(
        &self,
        requested: PagedLifecycleStamp,
        actor: &PagedDocumentGuard<'_>,
    ) -> Result<(OperationReservation, PagedLifecycleStamp), PagedLifecycleError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
        if self.stamp_for(actor) != requested {
            return Err(PagedLifecycleError::Changed);
        }
        if state.active_operation.is_some() {
            return Err(PagedLifecycleError::Busy);
        }
        let generation = self
            .0
            .operation_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        state.active_operation = Some(generation);
        Ok((
            OperationReservation {
                session: self.0.clone(),
                generation,
            },
            PagedLifecycleStamp {
                operation_generation: generation,
                ..requested
            },
        ))
    }
    pub fn new(opened: Box<PagedOpened>) -> Self {
        let recovered = opened.recovery_origin.is_some();
        let saved_state = (!recovered).then_some(opened.transcoded.document.saved_content_state());
        let state = PersistentState {
            path: opened.path.clone(),
            fingerprint: opened.fingerprint.clone(),
            saved_state,
            save_as_required: recovered,
            recovery_origin: opened.recovery_origin.clone(),
            encoding_failure: None,
            save_conflicts: Default::default(),
            save_cleanups: Default::default(),
            active_operation: None,
        };
        let generation_owner = Arc::new(());
        Self(Arc::new(SessionInner {
            recovery_platform: opened.transcoded.store.platform(),
            actor: Mutex::new(opened),
            state: Mutex::new(state),
            operation_generation: AtomicU64::new(0),
            recovery_config: Mutex::new(None),
            recovery: Mutex::new(None),
            recovery_discard: Mutex::new(None),
            recovery_cleanup_hold: Mutex::new(None),
            recovery_status: Mutex::new(Arc::new(Mutex::new(Default::default()))),
            failed_retirements: Mutex::new(Vec::new()),
            tail: Mutex::new(None),
            retired: Mutex::new(Vec::new()),
            source_mismatch: std::sync::atomic::AtomicBool::new(false),
            generation_owner: Mutex::new(generation_owner),
            spill: Mutex::new(PagedSpillState::default()),
            gate_generation: Mutex::new(0),
            gate_released: Condvar::new(),
        }))
    }

    pub fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn try_unwrap(self) -> Result<Box<PagedOpened>, Self> {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => Ok(inner.actor.into_inner().unwrap_or_else(|error| error.into_inner())),
            Err(inner) => Err(Self(inner)),
        }
    }
    pub fn lock_document(&self) -> Result<PagedDocumentGuard<'_>, PagedLifecycleError> {
        self.0
            .actor
            .lock()
            .map(PagedDocumentGuard)
            .map_err(|_| PagedLifecycleError::SourceUnavailable("document actor stopped".into()))
    }
    pub fn try_document(&self) -> Result<PagedDocumentGuard<'_>, PagedLifecycleError> {
        match self.0.actor.try_lock() {
            Ok(actor) => Ok(PagedDocumentGuard(actor)),
            Err(TryLockError::WouldBlock) => Err(PagedLifecycleError::Busy),
            Err(TryLockError::Poisoned(_)) => {
                Err(PagedLifecycleError::SourceUnavailable("document actor stopped".into()))
            }
        }
    }
    pub fn state(&self) -> Result<PagedLifecycleState, PagedLifecycleError> {
        let actor = self.try_document()?;
        let snapshot = actor.document().snapshot();
        let state = self
            .0
            .state
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
        Ok(PagedLifecycleState {
            stamp: PagedLifecycleStamp {
                document: snapshot.identity_token().0,
                revision: snapshot.revision,
                content: snapshot.content_state,
                source_generation: actor.source_generation(),
                operation_generation: self.0.operation_generation.load(Ordering::Acquire),
            },
            path: state.path.clone(),
            fingerprint: state.fingerprint.clone(),
            saved_state: state.saved_state,
            save_as_required: state.save_as_required,
            encoding: actor.0.transcoded.store.state.clone(),
            recovery_origin: state.recovery_origin.clone(),
        })
    }
    pub fn receipt_applies(&self, receipt: &PagedLifecycleReceipt) -> bool {
        self.lock_document()
            .is_ok_and(|actor| receipt.applies_to(self.stamp_for(&actor)))
    }
    pub fn stamp_for(&self, actor: &PagedDocumentGuard<'_>) -> PagedLifecycleStamp {
        let snapshot = actor.document().snapshot();
        PagedLifecycleStamp {
            document: snapshot.identity_token().0,
            revision: snapshot.revision,
            content: snapshot.content_state,
            source_generation: actor.source_generation(),
            operation_generation: self.0.operation_generation.load(Ordering::Acquire),
        }
    }
    pub fn path(&self) -> PathBuf {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).path.clone()
    }
    pub fn fingerprint(&self) -> Fingerprint {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fingerprint
            .clone()
    }
    pub fn saved_state(&self) -> Option<ContentStateId> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).saved_state
    }
    pub fn save_as_required(&self) -> bool {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).save_as_required
    }
    pub fn recovery_origin(&self) -> Option<PathBuf> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recovery_origin
            .clone()
    }
    pub fn mark_recovered(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.saved_state = None;
        self.0.operation_generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn require_save_as(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.save_as_required = true;
        self.0.operation_generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn configure_recovery(&self, root: PathBuf, platform: Arc<dyn LocalFileSystem>) {
        *self.0.recovery_config.lock().unwrap_or_else(|e| e.into_inner()) = Some((root, platform));
    }
    pub fn recovery_config(&self) -> Option<(PathBuf, Arc<dyn LocalFileSystem>)> {
        self.0.recovery_config.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn recovery_enabled(&self) -> bool {
        self.0.recovery_config.lock().is_ok_and(|config| config.is_some())
    }
    pub fn ensure_recovery(
        &self,
        actor: &PagedDocumentGuard<'_>,
        baseline: &PagedSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), PagedLifecycleError> {
        let Some((root, platform)) = self.recovery_config() else {
            return Ok(());
        };
        let mut recovery = self
            .0
            .recovery
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?;
        if recovery.is_none() {
            let status = Arc::new(Mutex::new(Default::default()));
            let created = crate::paged_recovery::PagedRecovery::create(
                &root,
                actor.0.transcoded.store.clone(),
                actor.0.recovery_origin.is_none().then(|| actor.0.path.clone()),
                baseline.clone(),
                platform,
                status.clone(),
                notify,
            )
            .map_err(PagedLifecycleError::SourceUnavailable)?;
            *self
                .0
                .recovery_status
                .lock()
                .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery status stopped".into()))? = status;
            *recovery = Some(created);
        }
        Ok(())
    }
    pub fn append_recovery_sources(
        &self,
        snapshot: &PagedSnapshot,
        edits: &[bareline_document::paged::SourceEdit],
        quota: u64,
    ) -> Result<(), PagedLifecycleError> {
        let mut recovery = self
            .0
            .recovery
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?;
        match recovery.as_mut() {
            Some(recovery) => recovery
                .append_sources(snapshot, edits, quota)
                .map_err(PagedLifecycleError::SourceUnavailable),
            None => Ok(()),
        }
    }
    pub fn append_recovery_history(
        &self,
        snapshot: &PagedSnapshot,
        edits: &[bareline_document::paged::HistorySourceEdit],
        quota: u64,
    ) -> Result<(), PagedLifecycleError> {
        let mut recovery = self
            .0
            .recovery
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?;
        match recovery.as_mut() {
            Some(recovery) => recovery
                .append_source_history(snapshot, edits, quota)
                .map_err(PagedLifecycleError::SourceUnavailable),
            None => Ok(()),
        }
    }
    pub fn protect_recovery_edits(
        &self,
        actor: &PagedDocumentGuard<'_>,
        baseline: &PagedSnapshot,
        snapshot: &PagedSnapshot,
        edits: &[crate::recovery::RecoveryEdit],
        rebuild: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), PagedLifecycleError> {
        if rebuild {
            *self
                .0
                .recovery
                .lock()
                .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))? = None;
        }
        self.ensure_recovery(actor, baseline, notify)?;
        let Some(mut recovery) = self.0.recovery.lock().ok() else {
            return Err(PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()));
        };
        let result = recovery
            .as_mut()
            .ok_or_else(|| PagedLifecycleError::SourceUnavailable("recovery unavailable".into()))?
            .append(snapshot, edits)
            .map_err(PagedLifecycleError::SourceUnavailable);
        if let Err(error) = &result
            && let Ok(status) = self.0.recovery_status.lock()
            && let Ok(mut status) = status.lock()
        {
            status.error = Some(error.to_string());
        }
        result
    }
    pub fn append_recovery_edits(
        &self,
        snapshot: &PagedSnapshot,
        edits: &[crate::recovery::RecoveryEdit],
    ) -> Result<(), PagedLifecycleError> {
        self.0
            .recovery
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?
            .as_mut()
            .ok_or_else(|| PagedLifecycleError::SourceUnavailable("recovery unavailable".into()))?
            .append(snapshot, edits)
            .map_err(PagedLifecycleError::SourceUnavailable)
    }
    fn retry_recovery_maintenance(
        &self,
        current: ContentStateId,
        cancellation: &Cancellation,
    ) -> Result<bool, PagedLifecycleError> {
        cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
        if let Some((_, platform)) = self.recovery_config() {
            let mut paths = std::mem::take(
                &mut *self
                    .0
                    .failed_retirements
                    .lock()
                    .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery retirement stopped".into()))?,
            );
            let mut failed = Vec::new();
            let mut failure = None;
            while let Some(path) = paths.pop() {
                if cancellation.check().is_err() {
                    failed.push(path);
                    failed.append(&mut paths);
                    *self
                        .0
                        .failed_retirements
                        .lock()
                        .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery retirement stopped".into()))? =
                        failed;
                    return Err(PagedLifecycleError::Cancelled);
                }
                if let Err(error) = crate::recovery::discard(&path, platform.as_ref()) {
                    failed.push(path);
                    failure = Some(error.to_string());
                }
            }
            *self
                .0
                .failed_retirements
                .lock()
                .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery retirement stopped".into()))? = failed;
            if let Some(error) = failure {
                if let Ok(status) = self.0.recovery_status.lock()
                    && let Ok(mut status) = status.lock()
                {
                    status.error = Some(error.clone());
                }
                return Err(PagedLifecycleError::SourceUnavailable(error));
            }
        }
        let rebuild = self.saved_state() != Some(current);
        if !rebuild
            && let Ok(status) = self.0.recovery_status.lock()
            && let Ok(mut status) = status.lock()
        {
            *status = Default::default();
        }
        Ok(rebuild)
    }
    fn detach_recovery_after_save(
        &self,
        actor: &PagedDocumentGuard<'_>,
        accepted: PagedLifecycleStamp,
        cancellation: &Cancellation,
    ) -> Result<Option<crate::paged_recovery::PagedRecovery>, PagedLifecycleError> {
        cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
        let state = self
            .0
            .state
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
        if self.stamp_for(actor) != accepted || state.saved_state != Some(accepted.content) {
            return Err(PagedLifecycleError::Changed);
        }
        let retired = self
            .0
            .recovery
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?
            .take();
        Ok(retired)
    }
    fn finish_recovery_retirement(
        &self,
        retired: Option<crate::paged_recovery::PagedRecovery>,
    ) -> Result<(), PagedLifecycleError> {
        let Some(retired) = retired else {
            return Ok(());
        };
        let path = retired.directory().to_path_buf();
        let retired_status = retired.status.clone();
        match retired.retire() {
            Ok(_) => {
                if let Ok(recovery) = self.0.recovery.lock()
                    && recovery.is_none()
                    && let Ok(mut current_status) = self.0.recovery_status.lock()
                    && Arc::ptr_eq(&*current_status, &retired_status)
                {
                    *current_status = Arc::new(Mutex::new(Default::default()));
                }
                Ok(())
            }
            Err(error) => {
                self.0
                    .failed_retirements
                    .lock()
                    .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery retirement stopped".into()))?
                    .push(path);
                if let Ok(recovery) = self.0.recovery.lock()
                    && recovery.is_none()
                    && let Ok(mut current_status) = self.0.recovery_status.lock()
                    && Arc::ptr_eq(&*current_status, &retired_status)
                    && let Ok(status) = retired_status.lock()
                {
                    let mut status = status.clone();
                    status.error = Some(error.clone());
                    *current_status = Arc::new(Mutex::new(status));
                }
                Err(PagedLifecycleError::SourceUnavailable(error))
            }
        }
    }
    pub fn recovery_status(&self) -> crate::paged_recovery::PagedRecoveryStatus {
        self.0
            .recovery_status
            .lock()
            .ok()
            .and_then(|status| status.lock().ok().map(|status| status.clone()))
            .unwrap_or_default()
    }
    pub fn resume_recovery_after_discard(&self) {
        {
            if let Ok(mut discard) = self.0.recovery_discard.lock() {
                *discard = None;
            }
        }
        {
            *self.0.recovery_status.lock().unwrap_or_else(|e| e.into_inner()) =
                Arc::new(Mutex::new(Default::default()));
        }
        {
            let _state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            self.0.operation_generation.fetch_add(1, Ordering::AcqRel);
        }
    }
    pub fn poll_recovery_discard(
        &self,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> crate::recovery_retirement::DiscardPoll {
        let mut discard = match self.0.recovery_discard.lock() {
            Ok(discard) => discard,
            Err(_) => {
                return crate::recovery_retirement::DiscardPoll::TombstoneFailed(
                    "Recovery discard state stopped".into(),
                );
            }
        };
        if discard.is_none() {
            let recovery = match self.0.recovery.try_lock() {
                Ok(mut recovery) => recovery.take(),
                Err(TryLockError::WouldBlock) => return crate::recovery_retirement::DiscardPoll::Pending,
                Err(TryLockError::Poisoned(_)) => {
                    return crate::recovery_retirement::DiscardPoll::TombstoneFailed("Recovery actor stopped".into());
                }
            };
            let origin = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .recovery_origin
                .take();
            if recovery.is_none() && origin.is_none() {
                return crate::recovery_retirement::DiscardPoll::Durable;
            }
            let platform = recovery
                .as_ref()
                .map_or_else(|| self.0.recovery_platform.clone(), |recovery| recovery.platform());
            let hold = {
                let mut current = self
                    .0
                    .recovery_cleanup_hold
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                current
                    .get_or_insert_with(crate::recovery_retirement::CleanupHold::new)
                    .clone()
            };
            *discard = Some(crate::recovery_retirement::DiscardTicket::request_with_cleanup_hold(
                crate::recovery_retirement::RecoveryOwnership {
                    recoveries: recovery.into_iter().collect(),
                    paths: origin.into_iter().collect(),
                    platform,
                },
                &hold,
                notify,
            ));
        }
        let outcome = discard.as_ref().unwrap().poll();
        if matches!(outcome, crate::recovery_retirement::DiscardPoll::TombstoneFailed(_)) {
            discard.as_ref().unwrap().retry();
        }
        outcome
    }
    pub fn lock_tail(&self) -> Result<PagedTailGuard<'_>, PagedLifecycleError> {
        self.0
            .tail
            .lock()
            .map(PagedTailGuard)
            .map_err(|_| PagedLifecycleError::SourceUnavailable("tail actor stopped".into()))
    }
    pub fn unlock_tail(
        &self,
        actor: &mut PagedDocumentGuard<'_>,
        tail: &mut PagedTailGuard<'_>,
    ) -> Result<Fingerprint, PagedLifecycleError> {
        let session = tail
            .0
            .as_ref()
            .ok_or_else(|| PagedLifecycleError::SourceUnavailable("monitoring is not active".into()))?;
        let fixed = session.freeze(&actor.0).map_err(classify_file_error)?;
        let previous = std::mem::replace(&mut *actor.0, fixed);
        let old_owner = std::mem::replace(
            &mut *self
                .0
                .generation_owner
                .lock()
                .map_err(|_| PagedLifecycleError::SourceUnavailable("generation owner stopped".into()))?,
            Arc::new(()),
        );
        let mut retired = self
            .0
            .retired
            .lock()
            .map_err(|_| PagedLifecycleError::SourceUnavailable("retired generation owner stopped".into()))?;
        retired.retain(|generation| Arc::strong_count(&generation.owner) > 1);
        retired.push(RetiredPagedGeneration {
            owner: old_owner,
            opened: previous,
            tail: tail.0.take(),
        });
        let fingerprint = actor.0.fingerprint.clone();
        let mut state = self.0.state.lock().unwrap_or_else(|error| error.into_inner());
        state.path = actor.0.path.clone();
        state.fingerprint = fingerprint.clone();
        Ok(fingerprint)
    }
    pub fn step_tail(
        &self,
        actor: &mut PagedDocumentGuard<'_>,
        tail: &mut PagedTailGuard<'_>,
        request: bool,
    ) -> Result<bool, PagedLifecycleError> {
        let tail = tail
            .0
            .as_mut()
            .ok_or_else(|| PagedLifecycleError::SourceUnavailable("monitoring is not active".into()))?;
        if request {
            tail.request(&actor.0.path).map_err(classify_file_error)?;
        }
        let changed = tail.step(&mut actor.0).map_err(classify_file_error)?;
        let mut state = self.0.state.lock().unwrap_or_else(|error| error.into_inner());
        state.path = actor.0.path.clone();
        state.fingerprint = actor.0.fingerprint.clone();
        state.saved_state = Some(actor.document().snapshot().content_state);
        Ok(changed)
    }
    pub fn source_changed(&self) -> bool {
        self.0.source_mismatch.load(Ordering::Acquire)
    }
    pub fn mark_source_changed(&self) {
        self.0.source_mismatch.store(true, Ordering::Release);
    }
    pub fn current_generation_owner(&self) -> Arc<()> {
        self.0
            .generation_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    pub fn configure_spill(
        &self,
        cache: PathBuf,
        platform: Arc<dyn LocalFileSystem>,
        options: crate::source::SourceOptions,
    ) {
        let mut spill = self.0.spill.lock().unwrap_or_else(|error| error.into_inner());
        spill.config = Some((cache, platform, options));
        spill.attempted = None;
    }
    pub fn take_spill_error(&self) -> Option<String> {
        self.0.spill.try_lock().ok().and_then(|mut spill| spill.error.take())
    }
    pub fn reserve_spill(&self, stamp: (u64, ContentStateId)) -> Option<PagedSpillReservation> {
        let mut spill = self.0.spill.try_lock().ok()?;
        if spill.pending || spill.attempted == Some(stamp) {
            return None;
        }
        let (cache, platform, options) = spill.config.clone()?;
        spill.pending = true;
        spill.attempted = Some(stamp);
        Some(PagedSpillReservation {
            cache,
            platform,
            options,
        })
    }
    pub fn finish_spill(&self, error: Option<String>) {
        if let Ok(mut spill) = self.0.spill.lock() {
            spill.pending = false;
            if error.is_some() {
                spill.error = error;
            }
        }
    }
    pub fn cancel_spill_reservation(&self) {
        if let Ok(mut spill) = self.0.spill.lock() {
            spill.pending = false;
            spill.attempted = None;
        }
    }
    pub fn spill_finished(&self) -> Result<bool, String> {
        let spill = self.0.spill.lock().map_err(|_| "Paged spill state stopped")?;
        if let Some(error) = &spill.error {
            return Err(error.clone());
        }
        Ok(spill.attempted.is_some() && !spill.pending)
    }
    pub fn signal_document_released(&self) {
        let mut generation = self.0.gate_generation.lock().unwrap_or_else(|e| e.into_inner());
        *generation = generation.wrapping_add(1);
        self.0.gate_released.notify_all();
    }
    fn gate_generation(&self) -> u64 {
        *self.0.gate_generation.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn wait_after(&self, observed: u64, timeout: std::time::Duration) -> bool {
        let guard = self.0.gate_generation.lock().unwrap_or_else(|e| e.into_inner());
        if *guard != observed {
            return true;
        }
        let (guard, _) = self
            .0
            .gate_released
            .wait_timeout_while(guard, timeout, |generation| *generation == observed)
            .unwrap_or_else(|e| e.into_inner());
        *guard != observed
    }
    #[cfg(test)]
    fn wait_after_with_entry_receipt(
        &self,
        observed: u64,
        timeout: std::time::Duration,
        entered: &std::sync::mpsc::SyncSender<()>,
    ) -> bool {
        let guard = self.0.gate_generation.lock().unwrap_or_else(|e| e.into_inner());
        entered.send(()).unwrap();
        if *guard != observed {
            return true;
        }
        let (guard, _) = self
            .0
            .gate_released
            .wait_timeout_while(guard, timeout, |generation| *generation == observed)
            .unwrap_or_else(|e| e.into_inner());
        *guard != observed
    }
    #[cfg(test)]
    fn spurious_gate_notify(&self) {
        let _guard = self.0.gate_generation.lock().unwrap_or_else(|e| e.into_inner());
        self.0.gate_released.notify_all();
    }
    pub fn read_handle(&self, snapshot: PagedSnapshot, generation: Arc<()>, views: Arc<()>) -> PagedReadHandle {
        PagedReadHandle {
            session: self.clone(),
            snapshot,
            generation,
            _views: views,
        }
    }
    pub fn encoding_failure(&self) -> Option<EncodingFailure> {
        self.0
            .state
            .lock()
            .ok()
            .and_then(|state| state.encoding_failure.clone())
    }
    pub fn take_save_conflict(&self) -> Option<SaveConflict> {
        self.0
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.save_conflicts.pop_front())
    }
    pub fn take_save_cleanup(&self) -> Option<SaveCleanup> {
        self.0
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.save_cleanups.pop_front())
    }
    fn execute_save(
        &self,
        requested: PagedLifecycleStamp,
        destination: PreparedDestination,
        copy_only: bool,
        platform: &dyn LocalFileSystem,
        cancellation: &Cancellation,
    ) -> PagedLifecycleReceipt {
        let reserved = (|| {
            cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
            let (store, source_generation, snapshot, reservation, accepted) = {
                let actor = self.lock_document()?;
                if destination.document != actor.document().snapshot().identity_token() {
                    return Err(PagedLifecycleError::Changed);
                }
                let (reservation, accepted) = self.reserve_operation(requested, &actor)?;
                (
                    actor.0.transcoded.store.clone(),
                    actor.source_generation(),
                    actor.document().snapshot(),
                    reservation,
                    accepted,
                )
            };
            Ok((store, source_generation, snapshot, reservation, accepted))
        })();
        let (store, source_generation, snapshot, reservation, accepted) = match reserved {
            Ok(reserved) => reserved,
            Err(error) => {
                return PagedLifecycleReceipt {
                    requested,
                    terminal: Err(error),
                };
            }
        };
        let terminal = (|| {
            let (source_path, recovered_origin) = {
                let state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
                (state.path.clone(), state.recovery_origin.is_some())
            };
            let _copy_source = if copy_only && !recovered_origin {
                guard_copy_source(&source_path, &destination.path, platform).map_err(classify_file_error)?
            } else {
                None
            };
            let policy = PagedSavePolicy {
                encoding: store.state.save_target,
                bom: store.state.bom,
                store,
                generation: source_generation,
            };
            let saved = match save_paged_to_cancellable(
                snapshot.clone(),
                &destination.path,
                &destination.condition,
                &policy,
                platform,
                cancellation,
            ) {
                Ok(saved) => saved,
                Err(error) => {
                    let mut state = self
                        .0
                        .state
                        .lock()
                        .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
                    if let Some(conflict) = error.save_conflict() {
                        state.save_conflicts.push_back(conflict);
                    }
                    return Err(classify_file_error(error));
                }
            };
            {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| PagedLifecycleError::SourceUnavailable("lifecycle state stopped".into()))?;
                if let Some(cleanup) = saved.cleanup.clone() {
                    state.save_cleanups.push_back(cleanup);
                }
                if !copy_only {
                    state.path = destination.path.clone();
                    state.fingerprint = saved.fingerprint.clone();
                    state.saved_state = Some(snapshot.content_state);
                    state.save_as_required = false;
                }
            }
            // The transaction is already committed. Synchronize the legacy
            // opened record when available, but never downgrade the durable
            // terminal result or discard cleanup authority if that mirror is
            // temporarily unavailable.
            if !copy_only && let Ok(mut actor) = self.lock_document() {
                actor.0.path = destination.path;
                actor.0.fingerprint = saved.fingerprint.clone();
            }
            Ok(PagedTerminalOutcome::Saved {
                fingerprint: saved.fingerprint,
                copy_only,
            })
        })();
        if let Ok(mut state) = self.0.state.lock() {
            state.encoding_failure = match &terminal {
                Err(PagedLifecycleError::Encoding(failure)) => Some(failure.clone()),
                _ => None,
            };
        }
        drop(reservation);
        PagedLifecycleReceipt {
            requested: accepted,
            terminal,
        }
    }
    pub fn execute(&self, command: PagedLifecycleCommand, cancellation: &Cancellation) -> PagedLifecycleReceipt {
        match command {
            PagedLifecycleCommand::Save {
                requested,
                destination,
                copy_only,
                platform,
            } => self.execute_save(requested, destination, copy_only, platform.as_ref(), cancellation),
            PagedLifecycleCommand::RetryRecovery { requested } => {
                let reserved = (|| {
                    cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
                    let actor = self.lock_document()?;
                    let (reservation, accepted) = self.reserve_operation(requested, &actor)?;
                    let rebuild = self.retry_recovery_maintenance(accepted.content, cancellation)?;
                    drop(actor);
                    Ok((reservation, accepted, rebuild))
                })();
                match reserved {
                    Ok((reservation, accepted, rebuild)) => {
                        drop(reservation);
                        PagedLifecycleReceipt {
                            requested: accepted,
                            terminal: Ok(PagedTerminalOutcome::RecoveryRetried { rebuild }),
                        }
                    }
                    Err(error) => PagedLifecycleReceipt {
                        requested,
                        terminal: Err(error),
                    },
                }
            }
            PagedLifecycleCommand::RetireRecovery { requested } => {
                let reserved = (|| {
                    cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
                    let actor = self.lock_document()?;
                    let (reservation, accepted) = self.reserve_operation(requested, &actor)?;
                    let retired = self.detach_recovery_after_save(&actor, accepted, cancellation)?;
                    drop(actor);
                    let terminal = self
                        .finish_recovery_retirement(retired)
                        .map(|()| PagedTerminalOutcome::RecoveryRetired);
                    Ok((reservation, accepted, terminal))
                })();
                match reserved {
                    Ok((reservation, accepted, terminal)) => {
                        drop(reservation);
                        PagedLifecycleReceipt {
                            requested: accepted,
                            terminal,
                        }
                    }
                    Err(error) => PagedLifecycleReceipt {
                        requested,
                        terminal: Err(error),
                    },
                }
            }
            PagedLifecycleCommand::Spill { requested, prepared } => {
                let reserved = (|| {
                    cancellation.check().map_err(|_| PagedLifecycleError::Cancelled)?;
                    let mut actor = self.lock_document()?;
                    let (reservation, accepted) = self.reserve_operation(requested, &actor)?;
                    actor
                        .document_mut()
                        .attach_spill(prepared)
                        .map_err(|error| PagedLifecycleError::SourceUnavailable(format!("spill: {error:?}")))?;
                    Ok((reservation, accepted))
                })();
                match reserved {
                    Ok((reservation, accepted)) => {
                        drop(reservation);
                        PagedLifecycleReceipt {
                            requested: accepted,
                            terminal: Ok(PagedTerminalOutcome::Spilled),
                        }
                    }
                    Err(error) => PagedLifecycleReceipt {
                        requested,
                        terminal: Err(error),
                    },
                }
            }
        }
    }
}

fn classify_file_error(error: FileError) -> PagedLifecycleError {
    match error {
        FileError::Cancelled => PagedLifecycleError::Cancelled,
        FileError::EncodingAt(failure) => PagedLifecycleError::Encoding(failure),
        FileError::Changed | FileError::Transcode(crate::codecs::disk::DiskError::Changed) => {
            PagedLifecycleError::Changed
        }
        other => {
            if let Some(conflict) = other.save_conflict() {
                PagedLifecycleError::Conflict(conflict)
            } else {
                PagedLifecycleError::Failed(other)
            }
        }
    }
}

pub fn commit_recovery_group(
    sessions: &[PagedSession],
    snapshots: &[PagedSnapshot],
    edits: &[crate::paged_recovery::group::GroupEdits<'_>],
    publication: u64,
    quota: u64,
    cancellation: &Cancellation,
) -> Result<(), PagedLifecycleError> {
    let mut guards = Vec::with_capacity(sessions.len());
    for session in sessions {
        guards.push(
            session
                .0
                .recovery
                .lock()
                .map_err(|_| PagedLifecycleError::SourceUnavailable("recovery actor stopped".into()))?,
        );
    }
    let mut recoveries = guards
        .iter_mut()
        .map(|recovery| {
            recovery
                .as_mut()
                .ok_or_else(|| PagedLifecycleError::SourceUnavailable("recovery unavailable".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    crate::paged_recovery::group::commit(&mut recoveries, snapshots, edits, publication, quota, cancellation)
        .map_err(PagedLifecycleError::SourceUnavailable)
}

/// Immutable, renderer-free snapshot/read capability. It retains the session,
/// source generation, recovery cleanup hold, and every historical source owner
/// needed by this captured root.
#[derive(Clone)]
pub struct PagedReadHandle {
    session: PagedSession,
    snapshot: PagedSnapshot,
    generation: Arc<()>,
    _views: Arc<()>,
}

impl PagedReadHandle {
    pub fn belongs_to(&self, session: &PagedSession) -> bool {
        self.session.same_session(session)
    }
    pub fn snapshot(&self) -> &PagedSnapshot {
        &self.snapshot
    }
    pub fn generation_owner(&self) -> Arc<()> {
        self.generation.clone()
    }
    pub fn original_store(&self) -> Result<DiskDecoded, String> {
        self.try_original_store().map_err(|error| error.to_string())
    }
    pub fn try_original_store(&self) -> Result<DiskDecoded, PagedLifecycleError> {
        let opened = self.session.try_document()?;
        let current = opened.document().snapshot();
        if !current.same_document(&self.snapshot) || current.content_state != self.snapshot.content_state {
            return Err(PagedLifecycleError::Changed);
        }
        Ok(opened.0.transcoded.store.clone())
    }
    pub fn actor_generation(&self) -> u64 {
        self.session.gate_generation()
    }
    pub fn wait_after(&self, observed: u64, timeout: std::time::Duration) -> bool {
        self.session.wait_after(observed, timeout)
    }
    pub fn resolve_page(&self, ticket: bareline_document::source::PageTicket) -> Result<bool, PagedLifecycleError> {
        self.resolve(ticket, false)
    }
    pub fn resolve_captured_page(
        &self,
        ticket: bareline_document::source::PageTicket,
    ) -> Result<bool, PagedLifecycleError> {
        self.resolve(ticket, true)
    }
    fn resolve(
        &self,
        ticket: bareline_document::source::PageTicket,
        historical: bool,
    ) -> Result<bool, PagedLifecycleError> {
        let referenced = self.snapshot.pieces().any(|piece| {
            let (source, range) = match piece {
                bareline_document::paged::PagedPiece::Original { source, range }
                | bareline_document::paged::PagedPiece::OriginalOwned { source, range, .. }
                | bareline_document::paged::PagedPiece::OwnedSource { source, range, .. } => (source, range),
                bareline_document::paged::PagedPiece::Inserted(_) => return false,
            };
            ticket.generation == source.generation()
                && ticket.page.checked_mul(source.page_size() as u64).is_some_and(|start| {
                    start < range.end && start.saturating_add(source.page_size() as u64) > range.start
                })
        });
        if !referenced {
            return Err(PagedLifecycleError::Changed);
        }
        let mut opened = match self.session.try_document() {
            Ok(opened) => opened,
            Err(PagedLifecycleError::Busy) => return Ok(false),
            Err(error) => return Err(error),
        };
        let current = opened.document().snapshot();
        if historical && !current.same_document(&self.snapshot) {
            drop(opened);
            let mut retired = match self.session.0.retired.try_lock() {
                Ok(retired) => retired,
                Err(TryLockError::WouldBlock) => return Ok(false),
                Err(_) => {
                    return Err(PagedLifecycleError::SourceUnavailable(
                        "captured generation owner stopped".into(),
                    ));
                }
            };
            let generation = retired
                .iter_mut()
                .find(|generation| {
                    generation
                        .opened
                        .transcoded
                        .document
                        .snapshot()
                        .same_document(&self.snapshot)
                })
                .ok_or_else(|| PagedLifecycleError::SourceUnavailable("captured generation is unavailable".into()))?;
            if self
                .snapshot
                .resolve_owned(ticket)
                .map_err(|error| PagedLifecycleError::SourceUnavailable(format!("captured owned source: {error:?}")))?
            {
                return Ok(true);
            }
            let handled = match generation.tail.as_mut() {
                Some(tail) => tail
                    .read_page(ticket)
                    .map_err(|error| source_read_error(error, &self.session.0.source_mismatch))?,
                None => false,
            };
            if !handled {
                generation
                    .opened
                    .transcoded
                    .source
                    .read_page(ticket)
                    .map_err(|error| source_read_error(error, &self.session.0.source_mismatch))?;
            }
            return Ok(true);
        }
        if !current.same_document(&self.snapshot)
            || (!historical && current.content_state != self.snapshot.content_state)
        {
            return Err(PagedLifecycleError::Changed);
        }
        if self
            .snapshot
            .resolve_owned(ticket)
            .map_err(|error| PagedLifecycleError::SourceUnavailable(format!("owned source: {error:?}")))?
        {
            return Ok(true);
        }
        let mut tail = match self.session.0.tail.try_lock() {
            Ok(tail) => tail,
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(_) => return Err(PagedLifecycleError::SourceUnavailable("tail worker stopped".into())),
        };
        let handled = match tail.as_mut() {
            Some(tail) => tail
                .read_page(ticket)
                .map_err(|error| source_read_error(error, &self.session.0.source_mismatch))?,
            None => false,
        };
        if !handled {
            opened
                .0
                .transcoded
                .source
                .read_page(ticket)
                .map_err(|error| source_read_error(error, &self.session.0.source_mismatch))?;
        }
        Ok(true)
    }
}

fn source_read_error(error: FileError, changed: &std::sync::atomic::AtomicBool) -> PagedLifecycleError {
    if matches!(
        error,
        FileError::Changed | FileError::Transcode(crate::codecs::disk::DiskError::Changed)
    ) {
        changed.store(true, Ordering::Release);
    }
    PagedLifecycleError::SourceUnavailable(format!("{error:?}"))
}

#[cfg(test)]
mod lifecycle_contract_tests {
    use super::*;
    use crate::{
        codecs::{Encoding, disk::DiskOptions},
        lifecycle::{
            DestinationCondition, DestinationConsent, PagedOpenRequest, SaveOperation, TranscodeOutcome,
            open_paged_encoded,
        },
        source::SourceOptions,
    };
    use bareline_document::{Budget, Edit, EditTransaction, TextOffset, paged::WindowPoll};
    use bareline_platform::{CommitReceipt, FileIdentity, PreparedCommit};
    use std::{
        fs::File,
        path::Path,
        sync::mpsc,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            File::open(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<FileIdentity> {
            let metadata = file.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: metadata.len(),
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            })
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, transaction: PreparedCommit) -> std::io::Result<CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> std::io::Result<()> {
            std::fs::rename(stage, target)
        }
    }

    fn replace_prefix(session: &PagedSession, removed_bytes: usize, insert: &str) {
        let mut actor = session.lock_document().unwrap();
        let snapshot = actor.document().snapshot();
        let read_budget = Budget::new(4096);
        let mut read = snapshot
            .begin_read(TextOffset(0)..TextOffset(snapshot.len()), snapshot.len(), &read_budget)
            .unwrap();
        let window = loop {
            match read.poll() {
                WindowPoll::Pending(ticket) => actor.0.transcoded.source.read_page(ticket).unwrap(),
                WindowPoll::Ready(window) => break window,
                _ => panic!("unexpected read outcome"),
            }
        };
        actor
            .document_mut()
            .apply_materialized(
                EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(0)..TextOffset(removed_bytes),
                        insert: insert.into(),
                    }],
                },
                &[window],
            )
            .unwrap();
    }

    fn save_as(session: &PagedSession, target: &Path, platform: Arc<dyn LocalFileSystem>) -> PagedLifecycleReceipt {
        let state = session.state().unwrap();
        session.execute(
            PagedLifecycleCommand::Save {
                requested: state.stamp,
                destination: PreparedDestination {
                    path: target.to_path_buf(),
                    condition: DestinationCondition::MustBeAbsent,
                    consent: DestinationConsent::NotRequired,
                    document: (state.stamp.document, state.stamp.revision.0),
                    operation: SaveOperation::SaveAs,
                },
                copy_only: false,
                platform,
            },
            &Cancellation::default(),
        )
    }

    struct GatedPlatform {
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl LocalFileSystem for GatedPlatform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            File::open(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<FileIdentity> {
            Platform.identity(file)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, transaction: PreparedCommit) -> std::io::Result<CommitReceipt> {
            self.entered.send(()).map_err(std::io::Error::other)?;
            self.release
                .lock()
                .map_err(|_| std::io::Error::other("release gate stopped"))?
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(std::io::Error::other)?;
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn cleanup_commit(&self, _: &mut CommitReceipt) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "controlled cleanup failure",
            ))
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> std::io::Result<()> {
            std::fs::rename(stage, target)
        }
    }

    #[test]
    fn headless_client_gets_exact_copy_and_generation_fenced_terminal_receipt() {
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-session-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let copy = root.join("copy.txt");
        let bytes = b"\xef\xbb\xbfalpha\r\nbeta\r\n";
        std::fs::write(&source, bytes).unwrap();
        let platform = Arc::new(Platform);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source.clone(),
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4096,
                    page_cache_bytes: 8192,
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture did not open")
        };
        let session = PagedSession::new(opened);
        let state = session.state().unwrap();
        let receipt = session.execute(
            PagedLifecycleCommand::Save {
                requested: state.stamp,
                destination: PreparedDestination {
                    path: copy.clone(),
                    condition: DestinationCondition::MustBeAbsent,
                    consent: DestinationConsent::NotRequired,
                    document: (state.stamp.document, state.stamp.revision.0),
                    operation: SaveOperation::SaveCopy,
                },
                copy_only: true,
                platform: platform.clone(),
            },
            &Cancellation::default(),
        );
        assert!(receipt.applies_to(session.state().unwrap().stamp));
        assert!(
            matches!(
                &receipt.terminal,
                Ok(PagedTerminalOutcome::Saved { copy_only: true, .. })
            ),
            "unexpected save-copy receipt: {:?}",
            receipt.terminal
        );
        assert_eq!(std::fs::read(copy).unwrap(), bytes);
        assert_eq!(session.path(), source);
        let mut stale = session.state().unwrap().stamp;
        stale.operation_generation = stale.operation_generation.wrapping_add(1);
        assert!(!receipt.applies_to(stale));
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encoding_diagnostic_tracks_latest_save_attempt_and_clears_on_success() {
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-session-encoding-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let target = root.join("saved.txt");
        std::fs::write(&source, "€\n".as_bytes()).unwrap();
        let platform = Arc::new(Platform);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source,
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4096,
                    page_cache_bytes: 8192,
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture did not open")
        };
        let session = PagedSession::new(opened);
        {
            let mut actor = session.lock_document().unwrap();
            actor.0.transcoded.store.state.convert_to(Encoding::Latin1);
        }

        let first = save_as(&session, &target, platform.clone());
        assert!(matches!(first.terminal, Err(PagedLifecycleError::Encoding(_))));
        let first_failure = session.encoding_failure().expect("first encoding diagnostic");
        let first_state = session.state().unwrap();
        assert!(first_failure.matches((first_state.stamp.document, first_state.stamp.revision.0)));

        replace_prefix(&session, "€".len(), "😀");
        let second = save_as(&session, &target, platform.clone());
        assert!(matches!(second.terminal, Err(PagedLifecycleError::Encoding(_))));
        let second_state = session.state().unwrap();
        let second_failure = session.encoding_failure().expect("replacement encoding diagnostic");
        assert!(second_failure.matches((second_state.stamp.document, second_state.stamp.revision.0)));
        assert_ne!(second_failure.revision, first_failure.revision);

        replace_prefix(&session, "😀".len(), "A");
        let saved = save_as(&session, &target, platform);
        assert!(
            matches!(&saved.terminal, Ok(PagedTerminalOutcome::Saved { .. })),
            "unexpected successful encoding retry receipt: {:?}",
            saved.terminal
        );
        assert!(session.encoding_failure().is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"A\n");
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_save_survives_concurrent_edit_and_retains_cleanup_authority() {
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-session-gated-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let target = root.join("saved.txt");
        let second = root.join("second.txt");
        let recovery_root = root.join("recovery");
        let bytes = b"\xef\xbb\xbfalpha\r\nbeta\r\n";
        std::fs::write(&source, bytes).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let platform = Arc::new(GatedPlatform {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        });
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source,
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4096,
                    page_cache_bytes: 8192,
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture did not open")
        };
        let session = PagedSession::new(opened);
        let recovery_platform = Arc::new(Platform);
        session.configure_recovery(recovery_root, recovery_platform.clone());
        let accepted_snapshot = session.lock_document().unwrap().document().snapshot();
        let accepted = session.state().unwrap();
        let accepted_stamp = accepted.stamp;
        let request = PreparedDestination {
            path: target.clone(),
            condition: DestinationCondition::MustBeAbsent,
            consent: DestinationConsent::NotRequired,
            document: (accepted_stamp.document, accepted_stamp.revision.0),
            operation: SaveOperation::SaveAs,
        };
        let saving = session.clone();
        let saving_platform = platform.clone();
        let thread = std::thread::spawn(move || {
            saving.execute(
                PagedLifecycleCommand::Save {
                    requested: accepted_stamp,
                    destination: request,
                    copy_only: false,
                    platform: saving_platform,
                },
                &Cancellation::default(),
            )
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("save never reached commit gate");

        let while_saving = session.state().unwrap();
        let busy = session.execute(
            PagedLifecycleCommand::Save {
                requested: while_saving.stamp,
                destination: PreparedDestination {
                    path: second,
                    condition: DestinationCondition::MustBeAbsent,
                    consent: DestinationConsent::NotRequired,
                    document: (while_saving.stamp.document, while_saving.stamp.revision.0),
                    operation: SaveOperation::SaveAs,
                },
                copy_only: false,
                platform: platform.clone(),
            },
            &Cancellation::default(),
        );
        assert!(matches!(busy.terminal, Err(PagedLifecycleError::Busy)));

        {
            let mut actor = session.lock_document().unwrap();
            let snapshot = actor.document().snapshot();
            let read_budget = Budget::new(4096);
            let mut read = snapshot
                .begin_read(TextOffset(0)..TextOffset(snapshot.len()), snapshot.len(), &read_budget)
                .unwrap();
            let window = loop {
                match read.poll() {
                    WindowPoll::Pending(ticket) => actor.0.transcoded.source.read_page(ticket).unwrap(),
                    WindowPoll::Ready(window) => break window,
                    _ => panic!("unexpected read outcome"),
                }
            };
            actor
                .document_mut()
                .apply_materialized(
                    EditTransaction {
                        base_revision: snapshot.revision,
                        edits: vec![Edit {
                            range: TextOffset(0)..TextOffset(1),
                            insert: "Z".into(),
                        }],
                    },
                    &[window],
                )
                .unwrap();
            let edited = actor.document().snapshot();
            session
                .protect_recovery_edits(
                    &actor,
                    &accepted_snapshot,
                    &edited,
                    &[crate::recovery::RecoveryEdit {
                        offset: 0,
                        removed: b"a".to_vec(),
                        inserted: b"Z".to_vec(),
                    }],
                    false,
                    Arc::new(|| {}),
                )
                .unwrap();
        }
        release_tx.send(()).unwrap();
        let receipt = thread.join().unwrap();
        assert!(matches!(
            &receipt.terminal,
            Ok(PagedTerminalOutcome::Saved { copy_only: false, .. })
        ));
        assert!(!session.receipt_applies(&receipt));
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        let current = session.state().unwrap();
        assert_eq!(current.path, target);
        assert_eq!(current.saved_state, Some(accepted_stamp.content));
        assert_ne!(current.stamp.content, accepted_stamp.content);
        assert!(session.take_save_cleanup().is_some());
        let retirement = session.execute(
            PagedLifecycleCommand::RetireRecovery {
                requested: receipt.requested,
            },
            &Cancellation::default(),
        );
        assert!(matches!(retirement.terminal, Err(PagedLifecycleError::Changed)));
        let recovery_directory = session
            .recovery_status()
            .directory
            .expect("newer recovery journal was detached");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let status = session.recovery_status();
            assert!(status.error.is_none(), "recovery failed: {:?}", status.error);
            if status.complete && status.durable.is_some() {
                break;
            }
            assert!(std::time::Instant::now() < until, "recovery did not become durable");
            std::thread::yield_now();
        }
        drop(session);
        let mut restored = crate::paged_recovery::restore(
            &recovery_directory,
            recovery_platform,
            Budget::new(64 * 1024 * 1024),
            Budget::new(16 * 1024 * 1024),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            crate::paged_recovery::preview(&mut restored, &Budget::new(4 * 1024 * 1024), &Cancellation::default(),)
                .unwrap(),
            "Zlpha\r\nbeta\r\n"
        );
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_release_gate_preserves_predicate_across_notification_orderings() {
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-session-gate-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        std::fs::write(&source, b"gate\n").unwrap();
        let platform = Arc::new(Platform);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source,
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(4 * 1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4096,
                    page_cache_bytes: 8192,
                },
            },
            platform,
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture did not open")
        };
        let session = PagedSession::new(opened);

        let observed = session.gate_generation();
        assert!(!session.wait_after(observed, std::time::Duration::from_millis(20)));

        let observed = session.gate_generation();
        session.signal_document_released();
        assert!(session.wait_after(observed, std::time::Duration::from_secs(1)));

        let observed = session.gate_generation();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let waiting = session.clone();
        let waiter = std::thread::spawn(move || {
            waiting.wait_after_with_entry_receipt(observed, std::time::Duration::from_secs(2), &entered_tx)
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("waiter never reached release gate");
        session.signal_document_released();
        assert!(waiter.join().unwrap());

        let observed = session.gate_generation();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let waiting = session.clone();
        let waiter = std::thread::spawn(move || {
            waiting.wait_after_with_entry_receipt(observed, std::time::Duration::from_millis(20), &entered_tx)
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("waiter never reached spurious-notify gate");
        session.spurious_gate_notify();
        assert!(!waiter.join().unwrap());

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready = Arc::new(std::sync::Barrier::new(2));
        let waiting = session.clone();
        let waiter_cancelled = cancelled.clone();
        let waiter_ready = ready.clone();
        let waiter = std::thread::spawn(move || {
            waiter_ready.wait();
            while !waiter_cancelled.load(Ordering::Acquire) {
                let observed = waiting.gate_generation();
                if waiter_cancelled.load(Ordering::Acquire) {
                    break;
                }
                waiting.wait_after(observed, std::time::Duration::from_secs(1));
            }
        });
        ready.wait();
        cancelled.store(true, Ordering::Release);
        session.signal_document_released();
        waiter.join().unwrap();

        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
