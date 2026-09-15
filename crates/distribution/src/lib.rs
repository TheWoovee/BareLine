// SPDX-License-Identifier: MPL-2.0
//! Distribution contracts. No network, registry, shell execution or document I/O.
pub mod cli;
pub mod importer;
pub mod trust;
pub mod update;

use std::path::{Path, PathBuf};

/// Caller probes the local executable-adjacent `bareline.portable` marker.
/// This function performs no I/O; all persistent stores must use this root.
pub fn data_root(executable: &Path, portable: bool, installed_root: &Path) -> Option<PathBuf> {
    if portable {
        Some(executable.parent()?.join("data"))
    } else {
        Some(installed_root.to_path_buf())
    }
}
