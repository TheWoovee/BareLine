// SPDX-License-Identifier: MPL-2.0
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cooperative cancellation. Blocking OS calls finish before the next checkpoint.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>, Option<Arc<dyn Fn() -> bool + Send + Sync>>);
impl Cancellation {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag, None)
    }
    pub(crate) fn with_check(check: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self(Arc::new(AtomicBool::new(false)), Some(check))
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), super::lifecycle::FileError> {
        if self.0.load(Ordering::Acquire) || self.1.as_ref().is_some_and(|check| check()) {
            Err(super::lifecycle::FileError::Cancelled)
        } else {
            Ok(())
        }
    }
}
