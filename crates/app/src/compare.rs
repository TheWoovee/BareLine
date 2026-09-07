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

/// Full immutable source captured for a comparison. Paged view prefixes must never
/// enter this contract; the handle resolves the captured piece tree on the worker.
#[derive(Clone)]
pub enum CompareInput {
    Resident(DocumentSnapshot),
    Paged(bareline_editor_surface::paged_view::PagedReadHandle),
    /// Display identity is independent from the immutable backing-source resolver.
    CapturedPaged(bareline_document::paged::PagedSnapshot,bareline_editor_surface::paged_view::PagedReadHandle),
}
impl CompareInput {
    pub fn same_document(&self, other:&Self)->bool {match (self,other) {
        (Self::Resident(a),Self::Resident(b))=>a.same_document(b),
        _=>self.paged_snapshot().zip(other.paged_snapshot()).is_some_and(|(a,b)|a.same_document(b)),
    }}
    pub fn current(&self, other:&Self)->bool {self.same_document(other)&&match (self,other) {
        (Self::Resident(a),Self::Resident(b))=>same(a,b),
        _=>self.paged_snapshot().zip(other.paged_snapshot()).is_some_and(|(a,b)|a.revision==b.revision&&a.content_state==b.content_state),
    }}
    fn paged_snapshot(&self)->Option<&bareline_document::paged::PagedSnapshot>{match self{Self::Paged(handle)=>Some(handle.snapshot()),Self::CapturedPaged(snapshot,_)=>Some(snapshot),Self::Resident(_)=>None}}
    pub fn len(&self)->usize {match self {Self::Resident(s)=>s.len(),Self::Paged(s)=>s.snapshot().len(),Self::CapturedPaged(s,_)=>s.len()}}
    pub fn is_empty(&self)->bool {self.len()==0}
    pub fn revision(&self)->bareline_document::Revision {match self {Self::Resident(s)=>s.revision,Self::Paged(s)=>s.snapshot().revision,Self::CapturedPaged(s,_)=>s.revision}}
}

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
    left: CompareInput,
    right: CompareInput,
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
                    let result = match (&job.left,&job.right) {
                        (CompareInput::Resident(left),CompareInput::Resident(right))=>bareline_diff::compare(left,right,&job.options,&job.cancel),
                        _=>compare_paged_inputs(&job.left,&job.right,&job.options,&job.cancel),
                    };
                    let _ = job.response.send(result);
                    (job.notify)();
                }
            })?;
        Ok(Self { sender })
    }
}
struct Pending {
    left: CompareInput,
    right: CompareInput,
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
    snapshots: Option<[CompareInput; 2]>,
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
        self.start_inputs(CompareInput::Resident(left),CompareInput::Resident(right),notify)
    }
    pub fn start_inputs(&mut self,left:CompareInput,right:CompareInput,notify:Arc<dyn Fn()+Send+Sync>)->Result<(),CompareError> {
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
        self.poll_inputs(&CompareInput::Resident(left.clone()),&CompareInput::Resident(right.clone()))
    }
    pub fn poll_inputs(&mut self,left:&CompareInput,right:&CompareInput)->bool {
        if let Some(pending) = &self.pending {
            if !pending.left.current(left) || !pending.right.current(right) {
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
                self.accept_inputs(result, [pending.left, pending.right], left, right);
            }
            _ => self.state = CompareState::Failed,
        }
        true
    }
    #[cfg(test)]
    fn accept(&mut self,result:CompareResult,captured:[DocumentSnapshot;2],left:&DocumentSnapshot,right:&DocumentSnapshot) {
        self.accept_inputs(result,captured.map(CompareInput::Resident),&CompareInput::Resident(left.clone()),&CompareInput::Resident(right.clone()));
    }
    fn accept_inputs(
        &mut self,
        result: CompareResult,
        captured: [CompareInput; 2],
        left: &CompareInput,
        right: &CompareInput,
    ) {
        if !captured[0].current(left)
            || !captured[1].current(right)
            || bareline_diff::is_stale(&result, left.revision(), right.revision())
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
        self.alignment = match (left,right) {(CompareInput::Resident(left),CompareInput::Resident(right))=>alignment(&result,left,right),_=>None};
        self.snapshots = Some(captured);
        self.result = Some(result);
    }
    /// Invoke with current snapshots before painting; stale hunks are never returned.
    pub fn visible_hunks(
        &mut self,
        left: &DocumentSnapshot,
        right: &DocumentSnapshot,
    ) -> &[DiffHunk] {
        self.visible_input_hunks(&CompareInput::Resident(left.clone()),&CompareInput::Resident(right.clone()))
    }
    pub fn visible_input_hunks(&mut self,left:&CompareInput,right:&CompareInput)->&[DiffHunk] {
        if self
            .snapshots
            .as_ref()
            .is_some_and(|s| !s[0].current(left) || !s[1].current(right))
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
    pub fn navigate_offset(&mut self,right:bool,offset:TextOffset)->Option<&DiffHunk> {
        let hunks=&self.result.as_ref()?.hunks;
        self.current=hunks.iter().enumerate().min_by_key(|(_,h)|{let range=if right{&h.right}else{&h.left};if offset<range.start{range.start.0-offset.0}else{offset.0.saturating_sub(range.end.0)}}).map(|(i,_)|i);
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

enum PagedResolver {
    Paged(bareline_editor_surface::paged_view::PagedReadHandle),
    Captured(bareline_editor_surface::paged_view::PagedReadHandle),
    Resident(DocumentSnapshot,bareline_document::source::SourcePublisher),
}
impl PagedResolver {
    fn prepare(input:&CompareInput)->Result<(bareline_document::paged::PagedSnapshot,Self),()> {
        use bareline_document::{Budget,source::{MemorySource,Generation,SourceKind}};
        match input {
            CompareInput::Paged(handle)=>Ok((handle.snapshot().clone(),Self::Paged(handle.clone()))),
            CompareInput::CapturedPaged(snapshot,handle)=>Ok((snapshot.clone(),Self::Captured(handle.clone()))),
            CompareInput::Resident(snapshot)=>{
                if !snapshot.is_complete(){return Err(());}
                let (source,publisher)=MemorySource::new(snapshot.len() as u64,Generation(1),SourceKind::Paged,64*1024,256*1024,Budget::new(512*1024)).map_err(|_|())?;
                let mut paged=bareline_document::paged::PagedSnapshot::utf8(source,0).map_err(|_|())?;
                paged.revision=snapshot.revision;
                Ok((paged,Self::Resident(snapshot.clone(),publisher)))
            }
        }
    }
    fn resolve(&self,ticket:bareline_document::source::PageTicket)->Result<bool,()> {
        match self {
            Self::Paged(handle)=>handle.resolve_page(ticket).map_err(|_|()),
            Self::Captured(handle)=>handle.resolve_captured_page(ticket).map_err(|_|()),
            Self::Resident(snapshot,publisher)=>{
                let start=(ticket.page as usize).checked_mul(64*1024).ok_or(())?;
                let end=start.saturating_add(64*1024).min(snapshot.len());
                let mut a=start;let mut b=end;
                // Pages split bytes, whereas snapshot reads require scalar boundaries.
                // Extend by at most three bytes at each edge, then publish exact page bytes.
                while snapshot.chunks(TextOffset(a)..TextOffset(snapshot.len())).is_err()&&a>0 {a-=1;if start-a>3{return Err(());}}
                while snapshot.chunks(TextOffset(a)..TextOffset(b)).is_err()&&b<snapshot.len(){b+=1;if b-end>3{return Err(());}}
                let text=snapshot.read(TextOffset(a)..TextOffset(b),64*1024+6).map_err(|_|())?;
                publisher.publish(ticket,&text.as_bytes()[start-a..end-a],ticket.generation).map_err(|_|())?;Ok(true)
            }
        }
    }
}
fn compare_paged_inputs(left:&CompareInput,right:&CompareInput,options:&CompareOptions,cancel:&CancelToken)->CompareResult {
    use bareline_diff::paged::{PagedCompareJob,PagedComparePoll,Side};
    let mut output=CompareResult {left_revision:left.revision(),right_revision:right.revision(),options:options.clone(),hunks:Vec::new(),completeness:CompareCompleteness::Failed,stats:Default::default()};
    let (Ok((l,lr)),Ok((r,rr)))=(PagedResolver::prepare(left),PagedResolver::prepare(right))else{return output;};
    let mut job=PagedCompareJob::new(l,r,options.clone(),cancel.clone());
    let mut retained=0usize;
    loop {
        match job.poll() {
            PagedComparePoll::Progress=>{},
            PagedComparePoll::Pending{side,ticket}=>match match side{Side::Left=>lr.resolve(ticket),Side::Right=>rr.resolve(ticket)} {
                Ok(true)=>{},Ok(false)=>std::thread::sleep(std::time::Duration::from_millis(1)),Err(())=>{output.completeness=CompareCompleteness::Unavailable;break;}
            },
            PagedComparePoll::Batch(batch)=>{
                let cost=batch.hunks.iter().fold(0usize,|sum,h|sum.saturating_add(std::mem::size_of::<DiffHunk>()).saturating_add(h.intraline.len()*std::mem::size_of::<bareline_diff::IntralineSpan>()));
                retained=retained.saturating_add(cost);
                if retained>options.limits.max_memory_bytes/2 {output.hunks.clear();output.completeness=CompareCompleteness::Unavailable;break;}
                output.hunks.extend(batch.hunks.iter().cloned());
            }
            PagedComparePoll::CoarseBlock(hunk)=>output.hunks.push(*hunk),
            PagedComparePoll::Finished(completeness)=>{output.completeness=completeness;break;},
            PagedComparePoll::Backpressure=>{output.completeness=CompareCompleteness::Failed;break;}
        }
    }
    if !matches!(output.completeness,CompareCompleteness::Exact|CompareCompleteness::Coarse(_)){output.hunks.clear();}
    output.stats.input_bytes=left.len().saturating_add(right.len());output.stats.peak_accounted_bytes=retained;output
}
fn input_text(input:&CompareInput,range:std::ops::Range<TextOffset>,cap:usize,cancel:&CancelToken)->Result<String,ApplyError> {
    use bareline_document::paged::WindowPoll;
    if range.end.0.saturating_sub(range.start.0)>cap{return Err(ApplyError::BudgetExceeded);}
    if let CompareInput::Resident(snapshot)=input{return snapshot.read(range,cap).map_err(|_|ApplyError::InvalidRange);}
    let (snapshot,resolver)=PagedResolver::prepare(input).map_err(|_|ApplyError::Unavailable)?;
    let budget=bareline_document::Budget::new(cap.saturating_mul(4));
    let mut request=snapshot.begin_read(range,cap,&budget).map_err(|_|ApplyError::BudgetExceeded)?;
    loop {if cancel.is_cancelled(){return Err(ApplyError::Unavailable);}match request.poll(){WindowPoll::Ready(window)=>return Ok(window.text().into()),WindowPoll::Pending(ticket)=>{if !resolver.resolve(ticket).map_err(|_|ApplyError::Unavailable)?{std::thread::sleep(std::time::Duration::from_millis(1));}},_=>return Err(ApplyError::Unavailable)}}
}
/// Stage a bounded mixed/paged hunk on the worker, then submit its single returned
/// transaction through the destination actor. Ignored text uses PR-025 policy.
pub fn prepare_input_merge(left:&CompareInput,right:&CompareInput,hunk:&DiffHunk,direction:Direction,options:&CompareOptions,policy:MergePolicy,cap:usize,cancel:&CancelToken)->Result<EditTransaction,ApplyError> {
    if left.revision()!=hunk.left_revision||right.revision()!=hunk.right_revision{return Err(ApplyError::Stale);}
    let a=input_text(left,hunk.left.clone(),cap,cancel)?;let b=input_text(right,hunk.right.clone(),cap,cancel)?;
    let (destination,range,source)=match direction{Direction::LeftToRight=>(right,hunk.right.clone(),&a),Direction::RightToLeft=>(left,hunk.left.clone(),&b)};
    let insert=if matches!(policy,MergePolicy::CopySelectedRange){source.clone()}else{
        let budget=bareline_document::Budget::new(cap.saturating_mul(12));
        let mut l=bareline_document::Document::from_utf8(&a,budget.clone(),budget.clone()).map_err(|_|ApplyError::BudgetExceeded)?;
        let mut r=bareline_document::Document::from_utf8(&b,budget.clone(),budget).map_err(|_|ApplyError::BudgetExceeded)?;
        let ls=l.snapshot();let rs=r.snapshot();let result=bareline_diff::compare(&ls,&rs,options,cancel);
        if !matches!(result.completeness,CompareCompleteness::Exact|CompareCompleteness::Coarse(_)){return Err(ApplyError::Unavailable);}
        let mut edits=Vec::new();for local in &result.hunks{edits.extend(bareline_diff::apply_hunk_with_policy(policy,direction,local,&ls,&rs,cap)?.edits);}
        let dest=match direction{Direction::LeftToRight=>&mut r,Direction::RightToLeft=>&mut l};
        if !edits.is_empty(){dest.apply(EditTransaction{base_revision:dest.snapshot().revision,edits}).map_err(|_|ApplyError::BudgetExceeded)?;}
        let result=dest.snapshot();result.read(TextOffset(0)..TextOffset(result.len()),cap).map_err(|_|ApplyError::BudgetExceeded)?
    };
    if cancel.is_cancelled(){return Err(ApplyError::Unavailable);}
    Ok(EditTransaction{base_revision:destination.revision(),edits:vec![bareline_document::Edit{range,insert}]})
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
    fn paged_worker_reads_beyond_first_window_and_utf8_page_boundaries() {
        let text=format!("{}late original\n","αβγ\n".repeat(20000));
        let changed=text.replace("late original","late changed");
        let left=Document::from_utf8(&text,Budget::new(2*1024*1024),Budget::new(0)).unwrap();
        let right=Document::from_utf8(&changed,Budget::new(2*1024*1024),Budget::new(0)).unwrap();
        let mut options=CompareOptions::default();options.limits.max_bytes_exact=1;
        let result=compare_paged_inputs(&CompareInput::Resident(left.snapshot()),&CompareInput::Resident(right.snapshot()),&options,&CancelToken::default());
        assert!(matches!(result.completeness,CompareCompleteness::Exact|CompareCompleteness::Coarse(_)));
        assert!(!result.hunks.is_empty());
        assert!(result.hunks.iter().any(|h|h.left.end.0>64*1024));
        let equal=compare_paged_inputs(&CompareInput::Resident(left.snapshot()),&CompareInput::Resident(left.snapshot()),&options,&CancelToken::default());
        assert!(equal.hunks.is_empty());
        assert_eq!(equal.completeness,CompareCompleteness::Exact);
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
