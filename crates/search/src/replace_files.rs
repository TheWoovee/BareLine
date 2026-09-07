// SPDX-License-Identifier: MPL-2.0
//! PR-026 preview and preparation for open documents. No disk writes or implicit saves.
use super::*;
use bareline_document::{
    Error as DocumentError,
    group::{MAX_GROUP_DOCUMENTS, UndoGroup},
    service::{
        Completion, DocumentService, GroupCompletion, GroupEdit, GroupMutation, GroupParticipant,
        Mutation, Scheduler, SubmitError,
    },
};
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Debug, PartialEq, Eq)]
pub enum PreviewError {
    Replace(ReplaceError),
    TooManyDocuments,
    DuplicateDocument,
    WrongDocument,
}
impl From<ReplaceError> for PreviewError {
    fn from(error: ReplaceError) -> Self {
        Self::Replace(error)
    }
}
pub struct PreviewChange {
    pub range: Range<TextOffset>,
    pub before: String,
    pub after: String,
    pub included: bool,
    edit: Edit,
}
pub struct OpenPreviewDocument {
    pub snapshot: DocumentSnapshot,
    pub included: bool,
    pub changes: Vec<PreviewChange>,
    service: DocumentService,
}
pub struct OpenReplacePreview {
    pub query: SearchQuery,
    pub replacement: String,
    documents: Vec<OpenPreviewDocument>,
}
impl OpenReplacePreview {
    pub fn documents(&self) -> &[OpenPreviewDocument] {
        &self.documents
    }
    pub fn set_document_included(&mut self, index: usize, included: bool) -> bool {
        if let Some(document) = self.documents.get_mut(index) {
            document.included = included;
            true
        } else {
            false
        }
    }
    pub fn set_match_included(&mut self, document: usize, index: usize, included: bool) -> bool {
        if let Some(change) = self
            .documents
            .get_mut(document)
            .and_then(|document| document.changes.get_mut(index))
        {
            change.included = included;
            true
        } else {
            false
        }
    }
    pub fn toggle_all(&mut self, included: bool) {
        for document in &mut self.documents {
            document.included = included;
            for change in &mut document.changes {
                change.included = included;
            }
        }
    }
    pub fn selected_matches(&self) -> usize {
        self.documents
            .iter()
            .filter(|d| d.included)
            .map(|d| d.changes.iter().filter(|c| c.included).count())
            .sum()
    }
    /// Consumes the reviewed exact edits. Cancellation before submission drops all staging;
    /// the document coordinator revalidates every source revision before any mutation.
    pub fn prepare(self, job: &SearchJob) -> Result<PreparedOpenReplace, PreviewError> {
        if job.is_cancelled() {
            return Err(ReplaceError::Cancelled.into());
        }
        let mut edits = Vec::new();
        let mut matches = 0;
        for document in self.documents {
            if job.is_cancelled() {
                return Err(ReplaceError::Cancelled.into());
            }
            if !document.included {
                continue;
            }
            let selected: Vec<_> = document
                .changes
                .into_iter()
                .filter(|change| change.included)
                .map(|change| change.edit)
                .collect();
            if selected.is_empty() {
                continue;
            }
            matches += selected.len();
            edits.push(GroupEdit {
                participant: GroupParticipant {
                    service: document.service,
                    snapshot: document.snapshot.clone(),
                },
                transaction: EditTransaction {
                    base_revision: document.snapshot.revision,
                    edits: selected,
                },
            });
        }
        if edits.is_empty() {
            return Err(ReplaceError::NoMatch.into());
        }
        Ok(PreparedOpenReplace { edits, matches })
    }
}
fn excerpt(text: &str) -> String {
    text.chars()
        .scan(0usize, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= 160).then_some(c)
        })
        .collect()
}
/// Worker-only phase one. Full search completion and bounded exact edits are required;
/// no service mutation is submitted here. All selected options remain inspectable.
pub fn preview_open_documents(
    targets: impl IntoIterator<Item = (DocumentService, DocumentSnapshot)>,
    query: &SearchQuery,
    replacement: &str,
    job: &SearchJob,
    ram_bytes: usize,
) -> Result<OpenReplacePreview, PreviewError> {
    let mut remaining = ram_bytes.min(MAX_RESULT_BYTES);
    if replacement.len() > MAX_PATTERN_BYTES {
        return Err(ReplaceError::StagingLimit.into());
    }
    let template = decode_replacement(replacement, query.mode)?;
    let mut documents: Vec<OpenPreviewDocument> = Vec::new();
    for (number, (service, snapshot)) in targets.into_iter().enumerate() {
        if job.is_cancelled() {
            return Err(ReplaceError::Cancelled.into());
        }
        if number >= MAX_GROUP_DOCUMENTS {
            return Err(PreviewError::TooManyDocuments);
        }
        if documents
            .iter()
            .any(|d| d.snapshot.same_document(&snapshot))
        {
            return Err(PreviewError::DuplicateDocument);
        }
        if !service.same_document(&snapshot) {
            return Err(PreviewError::WrongDocument);
        }
        let mut scoped = query.clone();
        scoped.results_ram_bytes = remaining;
        let results = scan(&snapshot, &scoped, job, |_| {});
        if results.completeness() != Completeness::Complete {
            return Err(ReplaceError::Incomplete.into());
        }
        if results.is_empty() {
            continue;
        }
        let transaction = results.prepare_replace_scoped(
            &snapshot,
            &template,
            remaining,
            ReplaceScope::All,
            job,
        )?;
        let mut changes = Vec::new();
        remaining = remaining
            .checked_sub(std::mem::size_of::<OpenPreviewDocument>())
            .ok_or(ReplaceError::StagingLimit)?;
        for edit in transaction.edits {
            if job.is_cancelled() {
                return Err(ReplaceError::Cancelled.into());
            }
            let mut end = (edit.range.start.0 + 160).min(edit.range.end.0);
            while !snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let before = snapshot
                .read(edit.range.start..TextOffset(end), 160)
                .map_err(|_| ReplaceError::Stale)?;
            let after = excerpt(&edit.insert);
            let needed = std::mem::size_of::<PreviewChange>()
                + edit.insert.capacity()
                + before.capacity()
                + after.capacity()
                + edit.range.end.0
                - edit.range.start.0;
            remaining = remaining
                .checked_sub(needed)
                .ok_or(ReplaceError::StagingLimit)?;
            changes.reserve_exact(1);
            changes.push(PreviewChange {
                range: edit.range.clone(),
                before,
                after,
                included: true,
                edit,
            });
        }
        documents.reserve_exact(1);
        documents.push(OpenPreviewDocument {
            snapshot,
            included: true,
            changes,
            service,
        });
    }
    if documents.is_empty() {
        return Err(ReplaceError::NoMatch.into());
    }
    Ok(OpenReplacePreview {
        query: query.clone(),
        replacement: replacement.into(),
        documents,
    })
}
pub struct PreparedOpenReplace {
    edits: Vec<GroupEdit>,
    matches: usize,
}
enum Pending {
    Single(Receiver<Completion>),
    Group(Receiver<GroupCompletion>),
}
pub struct OpenReplaceTicket {
    pending: Pending,
    matches: usize,
}
pub struct OpenReplaceCompletion {
    pub result: Result<Option<UndoGroup>, DocumentError>,
    pub snapshots: Vec<DocumentSnapshot>,
    pub matches_replaced: usize,
}
impl OpenReplaceTicket {
    pub fn try_recv(&self) -> Result<OpenReplaceCompletion, TryRecvError> {
        let (result, snapshots) = match &self.pending {
            Pending::Single(receiver) => {
                let completion = receiver.try_recv()?;
                (completion.result.map(|_| None), vec![completion.snapshot])
            }
            Pending::Group(receiver) => {
                let completion = receiver.try_recv()?;
                (completion.result.map(Some), completion.snapshots)
            }
        };
        let matches_replaced = if result.is_ok() { self.matches } else { 0 };
        Ok(OpenReplaceCompletion {
            result,
            snapshots,
            matches_replaced,
        })
    }
}
impl PreparedOpenReplace {
    /// Submission is the explicit phase-two boundary. Once accepted, the coordinator
    /// commits the complete group or none; dropping the ticket does not undo a commit.
    pub fn submit(
        mut self,
        scheduler: &Scheduler,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<OpenReplaceTicket, SubmitError> {
        let pending = if self.edits.len() == 1 {
            let edit = self.edits.pop().unwrap();
            Pending::Single(
                edit.participant
                    .service
                    .submit_with_notify(Mutation::Apply(edit.transaction), notify)
                    .map_err(|(error, _)| error)?,
            )
        } else {
            Pending::Group(
                scheduler
                    .submit_group(GroupMutation::Apply(self.edits), notify)
                    .map_err(|(error, _)| error)?,
            )
        };
        Ok(OpenReplaceTicket {
            pending,
            matches: self.matches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    fn target(scheduler: &Scheduler, text: &str) -> (DocumentService, DocumentSnapshot) {
        let document =
            Document::from_utf8(text, Budget::new(1024 * 1024), Budget::new(1024 * 1024)).unwrap();
        let snapshot = document.snapshot();
        (scheduler.document(document, 8), snapshot)
    }
    fn apply(prepared: PreparedOpenReplace, scheduler: &Scheduler) -> OpenReplaceCompletion {
        let (sender, receiver) = std::sync::mpsc::channel();
        let ticket = prepared
            .submit(
                scheduler,
                Some(Arc::new(move || {
                    let _ = sender.send(());
                })),
            )
            .unwrap();
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        ticket.try_recv().unwrap()
    }
    fn text(snapshot: &DocumentSnapshot) -> String {
        snapshot
            .read(TextOffset(0)..TextOffset(snapshot.len()), 4096)
            .unwrap()
    }
    #[test]
    fn preview_exclusions_apply_only_reviewed_matches_and_linked_undo_restores_both() {
        let scheduler = Scheduler::new(1, 8).unwrap();
        let a = target(&scheduler, "x x");
        let b = target(&scheduler, "x");
        let job = SearchJob::default();
        let mut preview = preview_open_documents(
            [a.clone(), b.clone()],
            &SearchQuery::literal("x"),
            "Y",
            &job,
            4096,
        )
        .unwrap();
        assert_eq!(preview.documents()[0].changes[0].before, "x");
        assert_eq!(preview.documents()[0].changes[0].after, "Y");
        preview.set_match_included(0, 1, false);
        assert_eq!(preview.selected_matches(), 2);
        let completion = apply(preview.prepare(&job).unwrap(), &scheduler);
        assert_eq!(completion.matches_replaced, 2);
        let group = completion.result.unwrap().unwrap();
        assert_eq!(text(&completion.snapshots[0]), "Y x");
        assert_eq!(text(&completion.snapshots[1]), "Y");
        let participants = completion
            .snapshots
            .into_iter()
            .map(|snapshot| GroupParticipant {
                service: if snapshot.same_document(&a.1) {
                    a.0.clone()
                } else {
                    b.0.clone()
                },
                snapshot,
            })
            .collect();
        let restored = scheduler
            .submit_group(
                GroupMutation::Undo {
                    group,
                    participants,
                },
                None,
            )
            .map_err(|(error, _)| error)
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(restored.result.is_ok());
        assert_eq!(text(&restored.snapshots[0]), "x x");
        assert_eq!(text(&restored.snapshots[1]), "x");
    }
    #[test]
    fn changed_revision_aborts_entire_group_and_cancelled_preview_never_submits() {
        let scheduler = Scheduler::new(1, 8).unwrap();
        let a = target(&scheduler, "x");
        let b = target(&scheduler, "x");
        let job = SearchJob::default();
        let preview = preview_open_documents(
            [a.clone(), b.clone()],
            &SearchQuery::literal("x"),
            "Y",
            &job,
            4096,
        )
        .unwrap();
        let changed =
            b.0.submit(Mutation::Apply(EditTransaction {
                base_revision: b.1.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(1),
                    insert: "z".into(),
                }],
            }))
            .map_err(|(error, _)| error)
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(changed.result.is_ok());
        let completion = apply(preview.prepare(&job).unwrap(), &scheduler);
        assert_eq!(completion.result, Err(DocumentError::StaleRevision));
        assert_eq!(completion.matches_replaced, 0);
        assert_eq!(text(&completion.snapshots[0]), "x");
        assert_eq!(text(&completion.snapshots[1]), "z");
        let preview =
            preview_open_documents([a.clone()], &SearchQuery::literal("x"), "Y", &job, 4096)
                .unwrap();
        job.cancel();
        assert!(matches!(
            preview.prepare(&job),
            Err(PreviewError::Replace(ReplaceError::Cancelled))
        ));
        assert!(matches!(
            preview_open_documents(
                [(a.0, changed.snapshot)],
                &SearchQuery::literal("z"),
                "Y",
                &SearchJob::default(),
                4096
            ),
            Err(PreviewError::WrongDocument)
        ));
    }
    #[test]
    fn excluded_document_and_bounded_capture_preview_are_exact() {
        let scheduler = Scheduler::new(1, 8).unwrap();
        let a = target(&scheduler, "ab12");
        let b = target(&scheduler, "ab34");
        let job = SearchJob::default();
        let mut query = SearchQuery::literal(r"(ab)(\d+)");
        query.mode = SearchMode::Regex;
        let mut preview = preview_open_documents([a.clone(), b], &query, "$2", &job, 4096).unwrap();
        preview.set_document_included(1, false);
        assert_eq!(preview.selected_matches(), 1);
        let completion = apply(preview.prepare(&job).unwrap(), &scheduler);
        assert_eq!(completion.result, Ok(None));
        assert_eq!(text(&completion.snapshots[0]), "12");
        assert!(matches!(
            preview_open_documents([a], &query, "$2", &job, 1),
            Err(PreviewError::Replace(ReplaceError::Incomplete))
        ));
    }
}
