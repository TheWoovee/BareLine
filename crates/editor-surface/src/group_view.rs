// SPDX-License-Identifier: MPL-2.0
//! UI-bound grouped edits. A ticket keeps all participating views busy until one completion.
use crate::{
    EditorSurface, SelectionHistory,
    power::{Bookmarks, PowerEdit, SelectionSet},
};
use bareline_document::{
    DocumentSnapshot,
    group::UndoGroup,
    service::{GroupCompletion, GroupEdit, GroupMutation, GroupParticipant, Scheduler},
};
use std::sync::mpsc::{Receiver, TryRecvError};

enum Direction {
    Apply,
    Undo,
    Redo,
}
struct ViewState {
    folds_before: Vec<std::ops::Range<usize>>,
    folds_after: Vec<std::ops::Range<usize>>,
    snapshot: DocumentSnapshot,
    before: SelectionSet,
    after: SelectionSet,
    bookmarks_before: Bookmarks,
    marks_before: crate::search_marks::SearchMarks,
    bookmarks_after: Bookmarks,
    marks_after: crate::search_marks::SearchMarks,
}
pub struct SurfaceGroup {
    receiver: Receiver<GroupCompletion>,
    states: Vec<ViewState>,
    direction: Direction,
    finished: bool,
}
impl SurfaceGroup {
    pub fn apply(
        scheduler: &Scheduler,
        views: &mut [&mut EditorSurface],
        prepared: Vec<(DocumentSnapshot, PowerEdit)>,
    ) -> Result<Self, String> {
        if views.len() != prepared.len() {
            return Err("Group views do not match prepared edits.".into());
        }
        let mut edits = Vec::with_capacity(views.len());
        let mut states = Vec::with_capacity(views.len());
        for (view, (snapshot, edit)) in views.iter().zip(prepared) {
            if view.busy()
                || view.read_only()
                || view.composition.is_some()
                || !view.snapshot.same_document(&snapshot)
                || view.snapshot.revision != snapshot.revision
            {
                return Err("A grouped document changed or is busy.".into());
            }
            let mut bookmarks_after = view.bookmarks.clone();
            bookmarks_after.map_edits(&edit.transaction);
            states.push(ViewState {
                folds_before: view.fold_anchors(),
                folds_after: view.mapped_folds(&edit.transaction),
                snapshot: snapshot.clone(),
                before: view.selection_set(),
                after: edit.selections,
                bookmarks_before: view.bookmarks.clone(),
                marks_before: view.search_marks.clone(),
                bookmarks_after,
                marks_after: view.search_marks.mapped(&edit.transaction),
            });
            edits.push(GroupEdit {
                participant: GroupParticipant {
                    service: view.service.as_ref().unwrap().clone(),
                    snapshot,
                },
                transaction: edit.transaction,
            });
        }
        Self::submit(
            scheduler,
            views,
            GroupMutation::Apply(edits),
            states,
            Direction::Apply,
        )
    }
    pub fn undo(
        scheduler: &Scheduler,
        views: &mut [&mut EditorSurface],
        group: UndoGroup,
    ) -> Result<Self, String> {
        Self::history(scheduler, views, group, false)
    }
    pub fn redo(
        scheduler: &Scheduler,
        views: &mut [&mut EditorSurface],
        group: UndoGroup,
    ) -> Result<Self, String> {
        Self::history(scheduler, views, group, true)
    }
    fn history(
        scheduler: &Scheduler,
        views: &mut [&mut EditorSurface],
        group: UndoGroup,
        redo: bool,
    ) -> Result<Self, String> {
        let mut participants = Vec::with_capacity(views.len());
        let mut states = Vec::with_capacity(views.len());
        for view in views.iter() {
            if view.busy() || view.read_only() || view.composition.is_some() {
                return Err("A linked document is busy.".into());
            }
            let entry = if redo {
                view.redo_selection.last()
            } else {
                view.undo_selection.last()
            }
            .filter(|entry| entry.group == Some(group))
            .ok_or("Linked undo requires all original documents at the same history boundary.")?;
            states.push(ViewState {
                folds_before: view.fold_anchors(),
                folds_after: if redo { entry.folds_after.clone() } else { entry.folds_before.clone() },
                snapshot: view.snapshot.clone(),
                before: view.selection_set(),
                after: if redo {
                    entry.after.clone()
                } else {
                    entry.before.clone()
                },
                bookmarks_before: view.bookmarks.clone(),
                marks_before: view.search_marks.clone(),
                marks_after: if redo {entry.marks_after.clone()}else{entry.marks_before.clone()},
                bookmarks_after: if redo {
                    entry.bookmarks_after.clone()
                } else {
                    entry.bookmarks_before.clone()
                },
            });
            participants.push(GroupParticipant {
                service: view.service.as_ref().unwrap().clone(),
                snapshot: view.snapshot.clone(),
            });
        }
        let mutation = if redo {
            GroupMutation::Redo {
                group,
                participants,
            }
        } else {
            GroupMutation::Undo {
                group,
                participants,
            }
        };
        Self::submit(
            scheduler,
            views,
            mutation,
            states,
            if redo {
                Direction::Redo
            } else {
                Direction::Undo
            },
        )
    }
    fn submit(
        scheduler: &Scheduler,
        views: &mut [&mut EditorSurface],
        mutation: GroupMutation,
        states: Vec<ViewState>,
        direction: Direction,
    ) -> Result<Self, String> {
        let notify = views.first().map(|view| view.notify.clone());
        let receiver = scheduler
            .submit_group(mutation, notify)
            .map_err(|(error, _)| format!("Grouped edit could not be queued: {error:?}"))?;
        for view in views {
            view.group_pending = true;
        }
        Ok(Self {
            receiver,
            states,
            direction,
            finished: false,
        })
    }
    /// Supply all original views. The event loop calls this only when notified.
    pub fn pump(&mut self, views: &mut [&mut EditorSurface]) -> Result<Option<UndoGroup>, String> {
        if self.finished {
            return Ok(None);
        }
        if self.states.iter().any(|state| {
            !views
                .iter()
                .any(|view| view.snapshot.same_document(&state.snapshot))
        }) {
            return Err(
                "A linked document view is missing; retain all views until completion.".into(),
            );
        }
        let completion = match self.receiver.try_recv() {
            Ok(completion) => completion,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                for view in views {
                    view.group_pending = false;
                }
                self.finished = true;
                return Err("Grouped document worker stopped.".into());
            }
        };
        self.finished = true;
        for state in &self.states {
            let view = views
                .iter_mut()
                .find(|view| view.snapshot.same_document(&state.snapshot))
                .unwrap();
            view.group_pending = false;
            if let Some(snapshot) = completion
                .snapshots
                .iter()
                .find(|snapshot| snapshot.same_document(&state.snapshot))
            {
                view.snapshot = snapshot.clone();
            }
            if let Ok(group) = completion.result {
                view.restore_fold_anchors(&state.folds_after);
                view.selection = state.after.primary();
                view.selections = state.after.clone();
                view.bookmarks = state.bookmarks_after.clone();
            view.search_marks = state.marks_after.clone();
                view.reveal_caret = true;
                match self.direction {
                    Direction::Apply => {
                        view.undo_selection.push(SelectionHistory {
                            folds_before: state.folds_before.clone(),
                            folds_after: state.folds_after.clone(),
                            before: state.before.clone(),
                            after: state.after.clone(),
                            bookmarks_before: state.bookmarks_before.clone(),
                            marks_before: state.marks_before.clone(),
                            bookmarks_after: state.bookmarks_after.clone(),
                            marks_after: state.marks_after.clone(),
                            group: Some(group),
                        });
                        view.redo_selection.clear();
                    }
                    Direction::Undo => {
                        if let Some(entry) = view.undo_selection.pop() {
                            view.redo_selection.push(entry);
                        }
                    }
                    Direction::Redo => {
                        if let Some(entry) = view.redo_selection.pop() {
                            view.undo_selection.push(entry);
                        }
                    }
                }
                view.error = None;
            } else {
                view.error = Some("Grouped edit was not applied.".into());
            }
        }
        completion
            .result
            .map(Some)
            .map_err(|error| format!("Grouped edit was not applied: {error:?}"))
    }
}
impl EditorSurface {
    pub fn linked_undo_group(&self) -> Option<UndoGroup> {
        self.undo_selection.last().and_then(|entry| entry.group)
    }
    pub fn linked_redo_group(&self) -> Option<UndoGroup> {
        self.redo_selection.last().and_then(|entry| entry.group)
    }
}
