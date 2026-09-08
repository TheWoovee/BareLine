// SPDX-License-Identifier: MPL-2.0
//! Atomic bounded multi-document commits. The caller owns all document mutation guards.
use crate::{Document, Error, PreparedEdit, Revision};
use std::sync::Arc;

// The specified 100-document replace scenario shares existing staging budgets.
pub const MAX_GROUP_DOCUMENTS: usize = 100;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoGroup(pub u64);
#[derive(Clone)]
pub(crate) struct GroupTag {
    id: UndoGroup,
    members: Arc<[u64]>,
}

fn members(documents: &[&mut Document]) -> Result<Arc<[u64]>, Error> {
    if documents.len() < 2 || documents.len() > MAX_GROUP_DOCUMENTS {
        return Err(Error::OutOfBounds);
    }
    let mut ids: Vec<_> = documents
        .iter()
        .map(|document| document.current.document_id)
        .collect();
    ids.sort_unstable();
    if ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(Error::WrongDocument);
    }
    Ok(ids.into())
}
pub fn commit(
    documents: &mut [&mut Document],
    mut prepared: Vec<PreparedEdit>,
) -> Result<UndoGroup, Error> {
    if documents.len() != prepared.len() {
        return Err(Error::OutOfBounds);
    }
    let members = members(documents)?;
    for (document, edit) in documents.iter_mut().zip(&prepared) {
        document.validate_prepared(edit)?;
        document
            .undo
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
    }
    let id = UndoGroup(crate::unique());
    for edit in &mut prepared {
        edit.entry.group = Some(GroupTag {
            id,
            members: members.clone(),
        });
    }
    // All validation, byte ownership and fallible allocations precede this loop.
    for (document, edit) in documents.iter_mut().zip(prepared) {
        document.commit_prepared_unchecked(edit);
    }
    Ok(id)
}
pub fn undo(documents: &mut [&mut Document], expected: UndoGroup) -> Result<(), Error> {
    move_history(documents, expected, false)
}
pub fn redo(documents: &mut [&mut Document], expected: UndoGroup) -> Result<(), Error> {
    move_history(documents, expected, true)
}
fn move_history(
    documents: &mut [&mut Document],
    expected: UndoGroup,
    redo: bool,
) -> Result<(), Error> {
    let members = members(documents)?;
    let mut revisions = Vec::with_capacity(documents.len());
    let mut changes = Vec::with_capacity(documents.len());
    for document in documents.iter_mut() {
        let history = if redo { &document.redo } else { &document.undo };
        let entry = history.last().ok_or(Error::EmptyHistory)?;
        let tag = entry.group.as_ref().ok_or(Error::LinkedUndoRequired)?;
        if tag.id != expected || tag.members != members {
            return Err(Error::LinkedUndoRequired);
        }
        revisions.push(Revision(
            document
                .current
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        ));
        changes.push(crate::change::AppliedChange::owned(
            document.current.document_id,
            document.current.revision,
            *revisions.last().expect("prepared revision"),
            document.current.content_state,
            if redo {
                entry.after_state
            } else {
                entry.before_state
            },
            if redo {
                crate::change::ChangeDirection::Redo
            } else {
                crate::change::ChangeDirection::Undo
            },
            &entry.edits,
            &document.bytes,
        )?);
        let destination = if redo {
            &mut document.undo
        } else {
            &mut document.redo
        };
        destination
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
    }
    for ((document, revision), change) in documents.iter_mut().zip(revisions).zip(changes) {
        document.current.applied_change = Some(change);
        let entry = if redo {
            document.redo.pop().unwrap()
        } else {
            document.undo.pop().unwrap()
        };
        document.current.metadata = if redo {
            entry.after_metadata.clone()
        } else {
            entry.before_metadata.clone()
        };
        document.current.root = if redo {
            entry.after.clone()
        } else {
            entry.before.clone()
        };
        document.current.content_state = if redo {
            entry.after_state
        } else {
            entry.before_state
        };
        document.current.revision = revision;
        if redo {
            document.undo.push(entry);
        } else {
            document.redo.push(entry);
        }
    }
    Ok(())
}
