// SPDX-License-Identifier: MPL-2.0
//! Screen reader support arrives with `accesskit_unix` and `accesskit_macos`
//! (PR-033); until then the shell's accessibility tree is built but not published.
use super::window::RawWindow;
use bareline_platform::accessibility::{AccessibilityAction, AccessibilitySnapshot, AccessibilityTextSource};
use std::sync::Arc;

pub struct Accessibility;
impl Accessibility {
    /// # Safety
    /// None for this stand-in, which keeps no handle; see `Platform::new`.
    pub unsafe fn new(
        _raw: RawWindow,
        _snapshot: AccessibilitySnapshot,
        _notify: Arc<dyn Fn() + Send + Sync>,
    ) -> std::result::Result<Self, &'static str> {
        eprintln!("event=accessibility_unavailable reason=no_platform_adapter");
        Ok(Self)
    }
    pub fn set_text_sources(&mut self, _sources: Vec<(u64, Arc<dyn AccessibilityTextSource>)>) {}
    pub fn update(&mut self, _snapshot: AccessibilitySnapshot) {}
    pub fn drain_actions(&mut self) -> Vec<AccessibilityAction> {
        Vec::new()
    }
}
