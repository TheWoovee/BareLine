// SPDX-License-Identifier: MPL-2.0
//! All-member leases. Caller locks all actors in document-ID order, prepares a
//! durable group marker for every future root, then publishes while guards remain held.
use crate::{
    Budget, BudgetClaim, Error,
    group::{MAX_GROUP_DOCUMENTS, UndoGroup},
    paged::{HistoryCommitLease, PagedDocument, PreparedSourceTransaction, SourceCommitLease},
};
use std::sync::Arc;
struct Members {
    ids: Box<[u64]>,
    _claim: BudgetClaim,
}
#[derive(Clone)]
pub(crate) struct PagedGroupTag {
    pub(crate) id: UndoGroup,
    members: Arc<Members>,
}
pub struct PagedSourceGroupLease<'a> {
    id: UndoGroup,
    members: Vec<SourceCommitLease<'a>>,
    _claim: BudgetClaim,
}
pub struct PagedHistoryGroupLease<'a> {
    id: UndoGroup,
    members: Vec<HistoryCommitLease<'a>>,
    _claim: BudgetClaim,
}
impl<'a> PagedSourceGroupLease<'a> {
    pub fn id(&self) -> UndoGroup {
        self.id
    }
    pub fn members(&self) -> &[SourceCommitLease<'a>] {
        &self.members
    }
    pub fn publish(self) -> UndoGroup {
        for member in self.members {
            member.publish();
        }
        self.id
    }
}
impl<'a> PagedHistoryGroupLease<'a> {
    pub fn id(&self) -> UndoGroup {
        self.id
    }
    pub fn members(&self) -> &[HistoryCommitLease<'a>] {
        &self.members
    }
    pub fn publish(self) -> UndoGroup {
        for member in self.members {
            member.publish();
        }
        self.id
    }
}
fn identities(documents: &[&mut PagedDocument], budget: &Budget) -> Result<Arc<Members>, Error> {
    if documents.len() < 2 || documents.len() > MAX_GROUP_DOCUMENTS {
        return Err(Error::OutOfBounds);
    }
    let claim = budget.claim(
        documents.len() * std::mem::size_of::<u64>()
            + std::mem::size_of::<Members>()
            + 2 * std::mem::size_of::<usize>(),
    )?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(documents.len())
        .map_err(|_| Error::BudgetExceeded)?;
    ids.extend(documents.iter().map(|doc| doc.current.document_id));
    ids.sort_unstable();
    if ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(Error::WrongDocument);
    }
    Ok(Arc::new(Members {
        ids: ids.into_boxed_slice(),
        _claim: claim,
    }))
}
pub fn lease_source_group<'a>(
    documents: &'a mut [&mut PagedDocument],
    prepared: Vec<PreparedSourceTransaction>,
    budget: &Budget,
) -> Result<PagedSourceGroupLease<'a>, Error> {
    if documents.len() != prepared.len() {
        return Err(Error::OutOfBounds);
    }
    let ids = identities(documents, budget)?;
    let id = UndoGroup(crate::unique());
    let claim = budget.claim(documents.len() * std::mem::size_of::<SourceCommitLease<'a>>())?;
    let mut members = Vec::new();
    members
        .try_reserve_exact(documents.len())
        .map_err(|_| Error::BudgetExceeded)?;
    for (document, prepared) in documents.iter_mut().zip(prepared) {
        let mut lease = document.lease_source_transaction(prepared)?;
        lease.tag_group(PagedGroupTag {
            id,
            members: ids.clone(),
        });
        members.push(lease);
    }
    Ok(PagedSourceGroupLease {
        id,
        members,
        _claim: claim,
    })
}
pub fn lease_history_group<'a>(
    documents: &'a mut [&mut PagedDocument],
    id: UndoGroup,
    undo: bool,
    budget: &Budget,
) -> Result<PagedHistoryGroupLease<'a>, Error> {
    let ids = identities(documents, budget)?;
    for document in documents.iter() {
        let entry = (if undo {
            document.undo.last()
        } else {
            document.redo.last()
        })
        .ok_or(Error::EmptyHistory)?;
        let tag = entry.group.as_ref().ok_or(Error::LinkedUndoRequired)?;
        if tag.id != id || tag.members.ids != ids.ids {
            return Err(Error::LinkedUndoRequired);
        }
    }
    let claim = budget.claim(documents.len() * std::mem::size_of::<HistoryCommitLease<'a>>())?;
    let mut members = Vec::new();
    members
        .try_reserve_exact(documents.len())
        .map_err(|_| Error::BudgetExceeded)?;
    for document in documents.iter_mut() {
        let prepared = document.prepare_source_history(undo, budget)?;
        members.push(document.lease_history_member(prepared, Some(id))?);
    }
    Ok(PagedHistoryGroupLease {
        id,
        members,
        _claim: claim,
    })
}
impl PagedDocument {
    pub fn history_group(&self, undo: bool) -> Option<UndoGroup> {
        (if undo { self.undo.last() } else { self.redo.last() })
            .and_then(|entry| entry.group.as_ref().map(|tag| tag.id))
    }
    pub fn history_group_members(&self, undo: bool) -> Option<&[u64]> {
        (if undo { self.undo.last() } else { self.redo.last() })
            .and_then(|entry| entry.group.as_ref().map(|tag| tag.members.ids.as_ref()))
    }
}
