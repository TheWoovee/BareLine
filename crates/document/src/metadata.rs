// SPDX-License-Identifier: MPL-2.0
//! Codec-neutral, bounded document policy state participating in undo/content identity.
use crate::Error;
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentMetadata(Arc<BTreeMap<String, String>>);
impl DocumentMetadata {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, Error> {
        let bytes = values
            .iter()
            .try_fold(0usize, |total, (key, value)| {
                total.checked_add(key.len())?.checked_add(value.len())
            })
            .ok_or(Error::BudgetExceeded)?;
        if values.len() > 64 || bytes > 64 * 1024 || values.keys().any(|key| key.is_empty()) {
            return Err(Error::BudgetExceeded);
        }
        Ok(Self(Arc::new(values)))
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }
    pub(crate) fn charge(&self) -> usize {
        self.0
            .iter()
            .map(|(key, value)| key.len() + value.len() + 128)
            .sum()
    }
}
