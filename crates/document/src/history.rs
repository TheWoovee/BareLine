// SPDX-License-Identifier: MPL-2.0
//! Neutral transaction intent and bounded history policy; timestamps are caller monotonic time.
use crate::{Error, TextOffset};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextOffset,
    pub caret: TextOffset,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditOrigin {
    Typing,
    Paste,
    Macro,
    MultiCursor,
    ReplaceAll,
    Extension,
    #[default]
    Command,
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EditMetadata {
    pub before: Vec<Selection>,
    pub after: Vec<Selection>,
    pub origin: EditOrigin,
    /// Changes on explicit cursor movement or any command boundary.
    pub boundary: u64,
    pub monotonic_ms: u64,
}
impl EditMetadata {
    pub(crate) fn validate(&self, before_len: usize, after_len: usize) -> Result<(), Error> {
        if self.before.len() > 1024 || self.after.len() > 1024 {
            return Err(Error::BudgetExceeded);
        }
        if self
            .before
            .iter()
            .any(|s| s.anchor.0 > before_len || s.caret.0 > before_len)
            || self
                .after
                .iter()
                .any(|s| s.anchor.0 > after_len || s.caret.0 > after_len)
        {
            return Err(Error::OutOfBounds);
        }
        Ok(())
    }
    pub(crate) fn follows(&self, previous: &Self, interval: u64) -> bool {
        self.origin == EditOrigin::Typing
            && previous.origin == EditOrigin::Typing
            && self.before.len() == 1
            && self.after.len() == 1
            && previous.after == self.before
            && self.boundary == previous.boundary
            && self
                .monotonic_ms
                .checked_sub(previous.monotonic_ms)
                .is_some_and(|gap| gap <= interval)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct HistoryPolicy {
    pub max_changes: usize,
    pub typing_interval_ms: u64,
}
impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            max_changes: 100_000,
            typing_interval_ms: 1000,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct HistoryStats {
    pub undo_changes: usize,
    pub redo_changes: usize,
    pub charged_payload_bytes: usize,
}
