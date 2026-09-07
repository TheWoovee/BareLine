// SPDX-License-Identifier: MPL-2.0
//! Compare Workspace state. Uses normal editor panes and PR-025 algorithms only.
use crate::views::{AlignmentBlock, AlignmentMap, ViewController};
use bareline_diff::{
    ApplyError, CancelToken, CoarseReason, CompareCompleteness, CompareOptions, CompareResult,
    DiffHunk, Direction, HunkId, MergePolicy,
};
use bareline_document::{DocumentSnapshot, EditTransaction, TextOffset};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompareOrigin {
    OpenDocument(String),
    Disk(String),
    LastSaved(String),
    Recovery(String),
    ExternalConflict(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompareSource {
    pub label: String,
    pub origin: CompareOrigin,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareState {
    AwaitingComparison,
    Running,
    Exact,
    Coarse(CoarseReason),
    Cancelled,
    Unavailable,
    Failed,
    Stale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareError {
    Busy,
    WorkerUnavailable,
    NoResult,
    Stale,
    MissingHunk,
    Apply(ApplyError),
    InvalidSession,
}
struct Request {
    left: DocumentSnapshot,
    right: DocumentSnapshot,
    options: CompareOptions,
    cancel: CancelToken,
    response: SyncSender<CompareResult>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
struct Worker {
    sender: SyncSender<Request>,
}
impl Worker {
    fn new() -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Request>(1);
        std::thread::Builder::new()
            .name("bareline-compare".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let result =
                        bareline_diff::compare(&job.left, &job.right, &job.options, &job.cancel);
                    let _ = job.response.send(result);
                    (job.notify)();
                }
            })?;
        Ok(Self { sender })
    }
}
struct Pending {
    left: DocumentSnapshot,
    right: DocumentSnapshot,
    cancel: CancelToken,
    receiver: Receiver<CompareResult>,
}
pub struct CompareController {
    pub sources: [CompareSource; 2],
    options: CompareOptions,
    pub state: CompareState,
    pub pause_automatic: bool,
    pub sync_horizontal: bool,
    result: Option<CompareResult>,
    snapshots: Option<[DocumentSnapshot; 2]>,
    pending: Option<Pending>,
    worker: Option<Worker>,
    current: Option<usize>,
    remembered: Option<HunkId>,
    alignment: Option<AlignmentMap>,
}
impl Drop for CompareController {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl CompareController {
    pub fn new(left: CompareSource, right: CompareSource) -> Self {
        Self {
            sources: [left, right],
            options: CompareOptions::default(),
            state: CompareState::AwaitingComparison,
            pause_automatic: false,
            sync_horizontal: false,
            result: None,
            snapshots: None,
            pending: None,
            worker: None,
            current: None,
            remembered: None,
            alignment: None,
        }
    }
    /// Bounded worker submission. Superseded work is cancelled and never painted.
    pub fn start(
        &mut self,
        left: DocumentSnapshot,
        right: DocumentSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CompareError> {
        self.remembered = self.current_hunk().map(|h| h.stable_id).or(self.remembered);
        self.cancel();
        self.result = None;
        self.snapshots = None;
        self.alignment = None;
        if self.worker.is_none() {
            self.worker = Some(Worker::new().map_err(|_| CompareError::WorkerUnavailable)?);
        }
        let (cancel, (sender, receiver)) = (CancelToken::default(), mpsc::sync_channel(1));
        let request = Request {
            left: left.clone(),
            right: right.clone(),
            options: self.options.clone(),
            cancel: cancel.clone(),
            response: sender,
            notify,
        };
        match self
            .worker
            .as_ref()
            .expect("worker")
            .sender
            .try_send(request)
        {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.state = CompareState::AwaitingComparison;
                return Err(CompareError::Busy);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.state = CompareState::Failed;
                return Err(CompareError::WorkerUnavailable);
            }
        }
        self.pending = Some(Pending {
            left,
            right,
            cancel,
            receiver,
        });
        self.state = CompareState::Running;
        Ok(())
    }
    pub fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.cancel();
        }
        if self.state == CompareState::Running {
            self.state = CompareState::Cancelled;
        }
    }
    pub fn invalidate(&mut self) {
        self.cancel();
        self.result = None;
        self.snapshots = None;
        self.alignment = None;
        self.state = CompareState::Stale;
    }
    pub fn options(&self) -> &CompareOptions {
        &self.options
    }
    pub fn set_options(&mut self, options: CompareOptions) {
        self.options = options;
        self.invalidate();
    }
    pub fn swap(&mut self) {
        self.sources.swap(0, 1);
        self.invalidate();
    }
    pub fn poll(&mut self, left: &DocumentSnapshot, right: &DocumentSnapshot) -> bool {
        if let Some(pending) = &self.pending {
            if !same(&pending.left, left) || !same(&pending.right, right) {
                self.invalidate();
                return true;
            }
        }
        let received = match self.pending.as_ref().map(|p| p.receiver.try_recv()) {
            None | Some(Err(TryRecvError::Empty)) => return false,
            Some(v) => v,
        };
        let pending = self.pending.take().expect("pending");
        match received {
            Ok(result) if !pending.cancel.is_cancelled() => {
                self.accept(result, [pending.left, pending.right], left, right);
            }
            _ => self.state = CompareState::Failed,
        }
        true
    }
    fn accept(
        &mut self,
        result: CompareResult,
        captured: [DocumentSnapshot; 2],
        left: &DocumentSnapshot,
        right: &DocumentSnapshot,
    ) {
        if !same(&captured[0], left)
            || !same(&captured[1], right)
            || bareline_diff::is_stale(&result, left.revision, right.revision)
        {
            self.state = CompareState::Stale;
            return;
        }
        self.state = match result.completeness {
            CompareCompleteness::Exact => CompareState::Exact,
            CompareCompleteness::Coarse(reason) => CompareState::Coarse(reason),
            CompareCompleteness::Cancelled => CompareState::Cancelled,
            CompareCompleteness::Unavailable => CompareState::Unavailable,
            CompareCompleteness::Failed => CompareState::Failed,
        };
        if !matches!(self.state, CompareState::Exact | CompareState::Coarse(_)) {
            self.result = None;
            return;
        }
        self.current = self
            .remembered
            .and_then(|id| result.hunks.iter().position(|h| h.stable_id == id))
            .or(if result.hunks.is_empty() {
                None
            } else {
                Some(0)
            });
        self.alignment = alignment(&result, left, right);
        self.snapshots = Some(captured);
        self.result = Some(result);
    }
    /// Invoke with current snapshots before painting; stale hunks are never returned.
    pub fn visible_hunks(
        &mut self,
        left: &DocumentSnapshot,
        right: &DocumentSnapshot,
    ) -> &[DiffHunk] {
        if self
            .snapshots
            .as_ref()
            .is_some_and(|s| !same(&s[0], left) || !same(&s[1], right))
        {
            self.invalidate();
        }
        self.result.as_ref().map_or(&[], |r| r.hunks.as_slice())
    }
    pub fn current_hunk(&self) -> Option<&DiffHunk> {
        self.current
            .and_then(|i| self.result.as_ref()?.hunks.get(i))
    }
    pub fn navigate(&mut self, previous: bool) -> Option<&DiffHunk> {
        let count = self.result.as_ref()?.hunks.len();
        if count == 0 {
            return None;
        }
        self.current = Some(match (self.current, previous) {
            (Some(0), true) | (None, true) => count - 1,
            (Some(i), true) => i - 1,
            (Some(i), false) => (i + 1) % count,
            (None, false) => 0,
        });
        self.current_hunk()
    }
    pub fn counter(&self) -> (usize, usize) {
        (
            self.current.map_or(0, |i| i + 1),
            self.result.as_ref().map_or(0, |r| r.hunks.len()),
        )
    }
    pub fn alignment(&self) -> Option<&AlignmentMap> {
        self.alignment.as_ref()
    }
    pub fn configure_views(&self, views: &mut ViewController) {
        views.split = true;
        views.ratio = 0.5;
        views.sync_vertical = true;
        views.sync_horizontal = self.sync_horizontal;
    }
    pub fn merge(
        &mut self,
        direction: Direction,
        left: &DocumentSnapshot,
        right: &DocumentSnapshot,
        max_bytes: usize,
        policy: MergePolicy,
    ) -> Result<EditTransaction, CompareError> {
        if self.visible_hunks(left, right).is_empty() {
            return Err(if self.state == CompareState::Stale {
                CompareError::Stale
            } else {
                CompareError::NoResult
            });
        }
        if !matches!(self.state, CompareState::Exact | CompareState::Coarse(_)) {
            return Err(CompareError::NoResult);
        }
        let h = self.current_hunk().ok_or(CompareError::MissingHunk)?;
        let transaction =
            bareline_diff::apply_hunk_with_policy(policy, direction, h, left, right, max_bytes)
                .map_err(CompareError::Apply)?;
        // Disable merge immediately while the actor applies this revision-bound transaction.
        self.invalidate();
        Ok(transaction)
    }
    pub fn status_text(&self) -> String {
        match self.state {
            CompareState::AwaitingComparison => "Ready to compare".into(),
            CompareState::Running => "Comparing…".into(),
            CompareState::Exact => format!("{} differences", self.counter().1),
            CompareState::Coarse(_) => {
                format!("Coarse comparison · {} differences", self.counter().1)
            }
            CompareState::Cancelled => "Comparison cancelled".into(),
            CompareState::Unavailable => "Source data unavailable".into(),
            CompareState::Failed => "Comparison failed".into(),
            CompareState::Stale => "Sources changed · Recompare".into(),
        }
    }
    pub fn session(&self) -> CompareSession {
        CompareSession {
            version: 1,
            sources: self.sources.clone(),
            options: SavedOptions::from(&self.options),
            sync_horizontal: self.sync_horizontal,
            pause_automatic: self.pause_automatic,
        }
    }
    pub fn restore(session: CompareSession) -> Result<Self, CompareError> {
        if session.version != 1 || session.sources.iter().any(|s| s.label.len() > 4096) {
            return Err(CompareError::InvalidSession);
        }
        let mut c = Self::new(session.sources[0].clone(), session.sources[1].clone());
        c.options = session.options.options()?;
        c.sync_horizontal = session.sync_horizontal;
        c.pause_automatic = session.pause_automatic;
        Ok(c)
    }
}
fn same(a: &DocumentSnapshot, b: &DocumentSnapshot) -> bool {
    a.same_document(b)
        && a.revision == b.revision
        && a.content_state == b.content_state
        && b.is_complete()
}
fn alignment(
    result: &CompareResult,
    left: &DocumentSnapshot,
    right: &DocumentSnapshot,
) -> Option<AlignmentMap> {
    let mut blocks = Vec::new();
    for h in &result.hunks {
        let l = h.left_line_hint?;
        let r = h.right_line_hint?;
        let end = |s: &DocumentSnapshot,
                   range: &std::ops::Range<TextOffset>,
                   start: usize|
         -> Option<u64> {
            if range.is_empty() {
                return Some(start as u64);
            }
            let line = s.line_at(range.end).ok()?;
            let extra = usize::from(s.line_range(line).ok()?.start < range.end);
            Some((line + extra) as u64)
        };
        blocks.push(AlignmentBlock {
            left: l as u64..end(left, &h.left, l)?,
            right: r as u64..end(right, &h.right, r)?,
        });
    }
    AlignmentMap::new(blocks).ok()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompareSession {
    pub version: u32,
    pub sources: [CompareSource; 2],
    pub options: SavedOptions,
    pub sync_horizontal: bool,
    pub pause_automatic: bool,
}
impl CompareSession {
    pub fn to_json(&self) -> Result<Vec<u8>, CompareError> {
        serde_json::to_vec(self).map_err(|_| CompareError::InvalidSession)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, CompareError> {
        if bytes.len() > 65536 {
            return Err(CompareError::InvalidSession);
        }
        let session: Self =
            serde_json::from_slice(bytes).map_err(|_| CompareError::InvalidSession)?;
        CompareController::restore(session.clone())?;
        Ok(session)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedOptions {
    pub whitespace: u8,
    pub ignore_blank_lines: bool,
    pub ignore_case: bool,
    pub ignore_eol_style: bool,
    pub ignore_encoding_bom: bool,
    pub normalize_tabs: bool,
}
impl From<&CompareOptions> for SavedOptions {
    fn from(o: &CompareOptions) -> Self {
        Self {
            whitespace: match o.whitespace {
                bareline_diff::Whitespace::Significant => 0,
                bareline_diff::Whitespace::TrimEdges => 1,
                bareline_diff::Whitespace::IgnoreAll => 2,
            },
            ignore_blank_lines: o.ignore_blank_lines,
            ignore_case: o.ignore_case,
            ignore_eol_style: o.ignore_eol_style,
            ignore_encoding_bom: o.ignore_encoding_bom,
            normalize_tabs: o.normalize_tabs,
        }
    }
}
impl SavedOptions {
    fn options(&self) -> Result<CompareOptions, CompareError> {
        Ok(CompareOptions {
            whitespace: match self.whitespace {
                0 => bareline_diff::Whitespace::Significant,
                1 => bareline_diff::Whitespace::TrimEdges,
                2 => bareline_diff::Whitespace::IgnoreAll,
                _ => return Err(CompareError::InvalidSession),
            },
            ignore_blank_lines: self.ignore_blank_lines,
            ignore_case: self.ignore_case,
            ignore_eol_style: self.ignore_eol_style,
            ignore_encoding_bom: self.ignore_encoding_bom,
            normalize_tabs: self.normalize_tabs,
            ..Default::default()
        })
    }
}
pub fn register_commands(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::*;
    for (id, title) in [
        ("compare.open", "Compare documents"),
        ("compare.next", "Next difference"),
        ("compare.previous", "Previous difference"),
        ("compare.swap", "Swap compare sources"),
        ("compare.recompare", "Recompare"),
        ("compare.cancel", "Cancel comparison"),
        ("compare.copyLeftToRight", "Copy left to right"),
        ("compare.copyRightToLeft", "Copy right to left"),
        ("compare.options", "Compare options"),
        ("compare.pauseAutomatic", "Pause automatic recompare"),
        ("compare.close", "Close compare"),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "Compare",
            shortcut: "",
            action: Action::Contributed(id),
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(10000), Budget::new(10000)).unwrap()
    }
    fn controller() -> CompareController {
        CompareController::new(
            CompareSource {
                label: "Left unsaved".into(),
                origin: CompareOrigin::OpenDocument("left".into()),
            },
            CompareSource {
                label: "Right disk".into(),
                origin: CompareOrigin::Disk("right.txt".into()),
            },
        )
    }
    #[test]
    fn stale_results_never_paint_and_merge_undo() {
        let left = doc("a\n");
        let mut right = doc("b\n");
        let ls = left.snapshot();
        let rs = right.snapshot();
        let mut c = controller();
        c.accept(
            bareline_diff::compare(&ls, &rs, &c.options, &CancelToken::default()),
            [ls.clone(), rs.clone()],
            &ls,
            &rs,
        );
        assert_eq!(c.counter(), (1, 1));
        let tx = c
            .merge(
                Direction::LeftToRight,
                &ls,
                &rs,
                1000,
                MergePolicy::PreserveIgnoredDestination,
            )
            .unwrap();
        right.apply(tx).unwrap();
        assert!(right.dirty());
        assert_eq!(c.state, CompareState::Stale);
        assert!(c.visible_hunks(&ls, &right.snapshot()).is_empty());
        right.undo().unwrap();
        assert!(!right.dirty());
    }
    #[test]
    fn old_revisions_and_cancelled_are_not_coarse() {
        let ls = doc("a").snapshot();
        let rs = doc("b").snapshot();
        let mut c = controller();
        let mut result = bareline_diff::compare(&ls, &rs, &c.options, &CancelToken::default());
        result.completeness = CompareCompleteness::Cancelled;
        c.accept(result, [ls.clone(), rs.clone()], &ls, &rs);
        assert_eq!(c.state, CompareState::Cancelled);
        assert!(c.visible_hunks(&ls, &rs).is_empty());
        let other = doc("a").snapshot();
        c.accept(
            bareline_diff::compare(&ls, &rs, &c.options, &CancelToken::default()),
            [ls, rs.clone()],
            &other,
            &rs,
        );
        assert_eq!(c.state, CompareState::Stale);
    }
    #[test]
    fn session_round_trip_preserves_options_sources() {
        let mut c = controller();
        c.options.ignore_case = true;
        c.options.whitespace = bareline_diff::Whitespace::IgnoreAll;
        c.sync_horizontal = true;
        let bytes = c.session().to_json().unwrap();
        let restored =
            CompareController::restore(CompareSession::from_json(&bytes).unwrap()).unwrap();
        assert_eq!(restored.sources, c.sources);
        assert!(restored.options.ignore_case && restored.sync_horizontal);
        assert_eq!(
            restored.options.whitespace,
            bareline_diff::Whitespace::IgnoreAll
        );
        assert!(CompareSession::from_json(b"{\"version\":99}").is_err());
    }
}
