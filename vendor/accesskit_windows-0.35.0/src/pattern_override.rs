// Copyright 2026 Bareline contributors.
// Licensed under the MIT or Apache-2.0 license, at your option.
//! Narrow application-owned pattern extension. The adapter retains fragment
//! navigation and events; a factory may replace a pattern on one existing node.
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::collections::HashMap;
use accesskit::{NodeId, TreeId};
use windows::{core::{IUnknown, Result}, Win32::{Foundation::HWND, System::Variant::VARIANT, UI::Accessibility::{IRawElementProviderSimple, UIA_PATTERN_ID, UIA_PROPERTY_ID}}};

pub trait PatternOverride: Send + Sync {
    fn pattern(&self, tree: TreeId, node: NodeId, pattern: UIA_PATTERN_ID,
        enclosing: IRawElementProviderSimple) -> Option<Result<IUnknown>>;
    fn property(&self, _tree: TreeId, _node: NodeId, _property: UIA_PROPERTY_ID) -> Option<Result<VARIANT>> { None }
}
type Factories = HashMap<isize, Weak<dyn PatternOverride>>;
fn factories() -> &'static Mutex<Factories> {
    static FACTORIES: OnceLock<Mutex<Factories>> = OnceLock::new();
    FACTORIES.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Keep the returned registration alive no longer than the HWND adapter.
/// Registry entries are weak: neither HWND reuse nor client-held COM ranges can
/// keep an application document alive after its owner drops the factory.
pub struct PatternRegistration { hwnd: isize, factory: Weak<dyn PatternOverride> }
impl Drop for PatternRegistration {
    fn drop(&mut self) {
        let mut entries = factories().lock().unwrap_or_else(|e| e.into_inner());
        if entries.get(&self.hwnd).is_some_and(|v| v.ptr_eq(&self.factory)) {
            entries.remove(&self.hwnd);
        }
    }
}
pub fn register_pattern_override(hwnd: HWND, factory: &Arc<dyn PatternOverride>) -> PatternRegistration {
    let factory = Arc::downgrade(factory);
    factories().lock().unwrap_or_else(|e| e.into_inner()).insert(hwnd.0 as isize, factory.clone());
    PatternRegistration { hwnd: hwnd.0 as isize, factory }
}
pub(crate) fn get(hwnd: HWND) -> Option<Arc<dyn PatternOverride>> {
    factories().lock().unwrap_or_else(|e| e.into_inner()).get(&(hwnd.0 as isize)).and_then(Weak::upgrade)
}
