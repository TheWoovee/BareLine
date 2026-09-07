// SPDX-License-Identifier: MPL-2.0
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cooperative cancellation. Blocking OS calls finish before the next checkpoint.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), super::lifecycle::FileError> {
        if self.0.load(Ordering::Acquire) {
            Err(super::lifecycle::FileError::Cancelled)
        } else {
            Ok(())
        }
    }
}
