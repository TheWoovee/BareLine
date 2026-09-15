// SPDX-License-Identifier: MPL-2.0
//! Compact acknowledged transitions. No document bytes are retained here.
use crate::{Budget, BudgetClaim, ContentStateId, Error, Revision, TextOffset, history::OwnedEdit, tree};
use std::{ops::Range, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeDirection {
    Edit,
    Undo,
    Redo,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactEdit {
    pub before: Range<TextOffset>,
    pub inserted_len: usize,
}
pub struct AppliedChange {
    pub document_id: u64,
    pub before_revision: Revision,
    pub after_revision: Revision,
    pub before_state: ContentStateId,
    pub after_state: ContentStateId,
    pub direction: ChangeDirection,
    edits: Box<[CompactEdit]>,
    _claim: BudgetClaim,
}
impl AppliedChange {
    pub fn charged_bytes(&self) -> usize {
        self.edits.len() * std::mem::size_of::<CompactEdit>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()
    }
    pub fn edits(&self) -> &[CompactEdit] {
        &self.edits
    }
    pub fn matches_before(&self, identity: (u64, u64), state: ContentStateId) -> bool {
        identity == (self.document_id, self.before_revision.0) && state == self.before_state
    }
    pub(crate) fn owned(
        document_id: u64,
        before_revision: Revision,
        after_revision: Revision,
        before_state: ContentStateId,
        after_state: ContentStateId,
        direction: ChangeDirection,
        edits: &[OwnedEdit],
        budget: &Budget,
    ) -> Result<Arc<Self>, Error> {
        Self::build(
            document_id,
            before_revision,
            after_revision,
            before_state,
            after_state,
            direction,
            edits.len(),
            edits.iter().map(|edit| {
                let (range, inserted) = if direction == ChangeDirection::Undo {
                    (&edit.after_range, &edit.inverse)
                } else {
                    (&edit.before_range, &edit.inserted)
                };
                CompactEdit {
                    before: TextOffset(range.start)..TextOffset(range.end),
                    inserted_len: tree::summary(inserted).bytes,
                }
            }),
            budget,
        )
    }
    pub(crate) fn build(
        document_id: u64,
        before_revision: Revision,
        after_revision: Revision,
        before_state: ContentStateId,
        after_state: ContentStateId,
        direction: ChangeDirection,
        count: usize,
        edits: impl Iterator<Item = CompactEdit>,
        budget: &Budget,
    ) -> Result<Arc<Self>, Error> {
        // Existing multi-caret commands admit 10,000 edits in one transaction.
        if count > 10_000 {
            return Err(Error::BudgetExceeded);
        }
        let bytes = count
            .checked_mul(std::mem::size_of::<CompactEdit>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()))
            .ok_or(Error::BudgetExceeded)?;
        let claim = budget.claim(bytes)?;
        let mut output = Vec::new();
        output.try_reserve_exact(count).map_err(|_| Error::BudgetExceeded)?;
        for edit in edits {
            if output.len() == count {
                return Err(Error::BudgetExceeded);
            }
            output.push(edit);
        }
        if output.len() != count {
            return Err(Error::OutOfBounds);
        }
        Ok(Arc::new(Self {
            document_id,
            before_revision,
            after_revision,
            before_state,
            after_state,
            direction,
            edits: output.into_boxed_slice(),
            _claim: claim,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Edit, EditTransaction};
    #[test]
    fn receipts_use_actual_ranges_and_reverse_history_without_text() {
        let bytes = Budget::new(1024 * 1024);
        let mut document = Document::from_utf8("abcdef", bytes.clone(), Budget::new(1024 * 1024)).unwrap();
        let before = document.snapshot();
        document
            .apply(EditTransaction {
                base_revision: before.revision,
                edits: vec![
                    Edit {
                        range: TextOffset(1)..TextOffset(3),
                        insert: "XYZ".into(),
                    },
                    Edit {
                        range: TextOffset(5)..TextOffset(6),
                        insert: String::new(),
                    },
                ],
            })
            .unwrap();
        let snapshot = document.snapshot();
        let applied = snapshot.applied_change().unwrap().clone();
        assert!(applied.matches_before(before.identity_token(), before.content_state));
        assert_eq!(
            applied.edits(),
            &[
                CompactEdit {
                    before: TextOffset(1)..TextOffset(3),
                    inserted_len: 3
                },
                CompactEdit {
                    before: TextOffset(5)..TextOffset(6),
                    inserted_len: 0
                }
            ]
        );
        document.undo().unwrap();
        let undone = document.snapshot();
        let receipt = undone.applied_change().unwrap();
        assert_eq!(receipt.direction, ChangeDirection::Undo);
        assert_eq!(receipt.after_state, before.content_state);
        assert_eq!(
            receipt.edits(),
            &[
                CompactEdit {
                    before: TextOffset(1)..TextOffset(4),
                    inserted_len: 2
                },
                CompactEdit {
                    before: TextOffset(6)..TextOffset(6),
                    inserted_len: 1
                }
            ]
        );
        document.redo().unwrap();
        assert_eq!(
            document.snapshot().applied_change().unwrap().direction,
            ChangeDirection::Redo
        );
        let revision = document.snapshot().revision;
        assert!(
            document
                .apply(EditTransaction {
                    base_revision: Revision(0),
                    edits: vec![]
                })
                .is_err()
        );
        assert_eq!(document.snapshot().revision, revision);
        assert_eq!(applied.direction, ChangeDirection::Edit);
    }
    #[test]
    fn ten_thousand_carets_have_complete_edit_and_undo_receipts() {
        let mut document = Document::from_utf8(
            &"a".repeat(10_000),
            Budget::new(64 * 1024 * 1024),
            Budget::new(64 * 1024 * 1024),
        )
        .unwrap();
        let before = document.snapshot();
        let edits = (0..10_000)
            .map(|offset| Edit {
                range: TextOffset(offset)..TextOffset(offset),
                insert: "x".into(),
            })
            .collect();
        document
            .apply(EditTransaction {
                base_revision: before.revision,
                edits,
            })
            .unwrap();
        let edited = document.snapshot();
        let receipt = edited.applied_change().unwrap();
        assert_eq!(receipt.edits().len(), 10_000);
        assert_eq!(edited.len(), 20_000);
        assert!(
            receipt
                .edits()
                .iter()
                .enumerate()
                .all(
                    |(offset, edit)| edit.before == (TextOffset(offset)..TextOffset(offset))
                        && edit.inserted_len == 1
                )
        );
        document.undo().unwrap();
        let undone = document.snapshot();
        let receipt = undone.applied_change().unwrap();
        assert_eq!(receipt.direction, ChangeDirection::Undo);
        assert_eq!(receipt.edits().len(), 10_000);
        assert!(receipt.edits().iter().enumerate().all(|(offset, edit)| edit.before
            == (TextOffset(offset * 2)..TextOffset(offset * 2 + 1))
            && edit.inserted_len == 0));
        assert_eq!(undone.content_state, before.content_state);
        assert_eq!(undone.len(), 10_000);
    }
}
