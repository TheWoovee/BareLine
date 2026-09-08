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
    pub charged_capacity_bytes: usize,
}

#[derive(Clone)]
pub(crate) struct Charge(Vec<std::sync::Arc<crate::Reservation>>);
impl Charge {
    pub(crate) fn new(reservation: crate::Reservation) -> Self {
        Self(vec![std::sync::Arc::new(reservation)])
    }
    pub(crate) fn reference_bytes(&self) -> usize {
        self.0.len() * std::mem::size_of::<std::sync::Arc<crate::Reservation>>()
    }
    pub(crate) fn bytes(&self) -> usize {
        self.0.iter().map(|claim| claim.bytes).sum()
    }
    pub(crate) fn add(&mut self, reservation: crate::Reservation) {
        self.0.push(std::sync::Arc::new(reservation));
    }
    pub(crate) fn merge(&mut self, mut other: Self) {
        self.0.append(&mut other.0);
    }
}
#[derive(Clone)]
pub(crate) struct OwnedEdit {
    pub(crate) before_range: std::ops::Range<usize>,
    pub(crate) after_range: std::ops::Range<usize>,
    pub(crate) inverse: crate::tree::Root,
    pub(crate) inserted: crate::tree::Root,
}

/// History slots own their allocation charge independently of entry payloads.
/// Clearing/trimming entries does not release capacity. No implicit growth in push.
pub(crate) struct HistoryStack<T> {
    entries: Vec<T>,
    budget: crate::Budget,
    capacity_charge: Option<crate::Reservation>,
}
impl<T> HistoryStack<T> {
    pub(crate) fn new(budget: crate::Budget) -> Self {
        Self {
            entries: Vec::new(),
            budget,
            capacity_charge: None,
        }
    }
    pub(crate) fn from_vec(entries: Vec<T>, budget: crate::Budget) -> Result<Self, Error> {
        let bytes = entries
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(Error::BudgetExceeded)?;
        let capacity_charge = if bytes == 0 {
            None
        } else {
            Some(budget.reserve(bytes)?)
        };
        Ok(Self {
            entries,
            budget,
            capacity_charge,
        })
    }
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.capacity_charge
            .as_ref()
            .map_or(0, |charge| charge.bytes)
    }
    pub(crate) fn try_reserve(&mut self, additional: usize) -> Result<(), Error> {
        self.try_reserve_exact(additional)
    }
    pub(crate) fn try_reserve_exact(&mut self, additional: usize) -> Result<(), Error> {
        let requested = self
            .entries
            .len()
            .checked_add(additional)
            .ok_or(Error::BudgetExceeded)?;
        if requested <= self.entries.capacity() {
            return Ok(());
        }
        // Geometric growth avoids copying the entire history for every edit.
        let requested = requested
            .max(
                self.entries
                    .capacity()
                    .checked_mul(2)
                    .ok_or(Error::BudgetExceeded)?,
            )
            .max(1);
        // Reserve the entire new allocation while the old allocation remains live.
        // Moving entries and swapping storage cannot fail after this preparation.
        let bytes = requested
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(Error::BudgetExceeded)?;
        let mut charge = self.budget.reserve(bytes)?;
        let mut replacement = Vec::new();
        replacement
            .try_reserve_exact(requested)
            .map_err(|_| Error::BudgetExceeded)?;
        let actual = replacement
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(Error::BudgetExceeded)?;
        if actual > bytes {
            let mut extra = self.budget.reserve(actual - bytes)?;
            charge.bytes += extra.bytes;
            extra.bytes = 0;
        }
        replacement.append(&mut self.entries);
        self.entries = replacement;
        self.capacity_charge = Some(charge);
        Ok(())
    }
    pub(crate) fn push(&mut self, entry: T) {
        assert!(
            self.entries.len() < self.entries.capacity(),
            "history growth must be admitted before publication"
        );
        self.entries.push(entry);
    }
    pub(crate) fn pop(&mut self) -> Option<T> {
        self.entries.pop()
    }
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
    pub(crate) fn drain<R: std::ops::RangeBounds<usize>>(
        &mut self,
        range: R,
    ) -> std::vec::Drain<'_, T> {
        self.entries.drain(range)
    }
}
impl<T> std::ops::Deref for HistoryStack<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.entries
    }
}
impl<T> std::ops::DerefMut for HistoryStack<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.entries
    }
}
impl<'a, T> IntoIterator for &'a HistoryStack<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
#[cfg(test)]
mod capacity_tests {
    use super::*;
    #[test]
    fn trimmed_empty_slots_remain_charged_and_admitted_push_never_grows() {
        let budget = crate::Budget::new(4096);
        let mut stack = HistoryStack::<u64>::new(budget.clone());
        stack.try_reserve_exact(16).unwrap();
        let retained = stack.capacity_bytes();
        assert!(retained >= 128);
        for value in 0..16 {
            stack.push(value);
        }
        stack.drain(..16);
        assert!(stack.is_empty());
        assert_eq!(budget.used(), retained);
        let full = budget.claim(budget.limit() - budget.used()).unwrap();
        stack.push(7);
        assert_eq!(stack.pop(), Some(7));
        assert!(matches!(
            stack.try_reserve_exact(32),
            Err(Error::BudgetExceeded)
        ));
        assert!(stack.is_empty());
        assert_eq!(stack.capacity_bytes(), retained);
        drop(full);
        drop(stack);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn sequential_admission_has_logarithmic_capacity_growth() {
        let budget = crate::Budget::new(8192);
        let mut stack = HistoryStack::<u64>::new(budget.clone());
        let mut previous = 0;
        let mut growths = 0;
        for value in 0..64 {
            stack.try_reserve_exact(1).unwrap();
            if stack.capacity_bytes() != previous {
                growths += 1;
                previous = stack.capacity_bytes();
            }
            stack.push(value);
        }
        assert!(growths <= 7);
        assert_eq!(stack.len(), 64);
        drop(stack);
        assert_eq!(budget.used(), 0);
    }
}
