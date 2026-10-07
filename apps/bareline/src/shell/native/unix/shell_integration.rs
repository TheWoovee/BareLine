// SPDX-License-Identifier: MPL-2.0
//! Reveal, terminal, recent items, the jump list and the tray icon.
use bareline_platform::{Capability, Unsupported};
use std::path::Path;

fn unavailable(capability: Capability) -> String {
    Unsupported { capability }.to_string()
}
pub fn reveal(_path: &Path) -> Result<(), String> {
    Err(unavailable(Capability::Shell))
}
/// Only Windows searches the launch directory for libraries.
pub fn harden_process_search_paths() -> Result<(), String> {
    Ok(())
}
/// Network locations on these systems are mounts with ordinary absolute paths;
/// whether a path lives on one is a property of its mount, not its spelling.
pub fn is_network_path(_path: &Path) -> bool {
    false
}
pub fn open_terminal(_directory: &Path) -> Result<(), String> {
    Err(unavailable(Capability::Shell))
}
/// Desktop recent-items integration is not built yet; Bareline's own Recent
/// Files list is unaffected.
pub fn add_recent(_path: &Path, _portable: bool, _enabled: bool) {}
/// Jump lists are a Windows taskbar feature.
pub fn initialize_jump_list(_portable: bool) -> Result<(), String> {
    Ok(())
}
#[allow(
    dead_code,
    reason = "the tray icon is unsupported here, so it never reports an action"
)]
#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    Restore,
    New,
    Open,
    Find,
    Exit,
}
pub struct TrayIcon;
impl TrayIcon {
    pub fn new(_window: super::RawWindow) -> Result<Self, String> {
        Err(unavailable(Capability::Tray))
    }
}
