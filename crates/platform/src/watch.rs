// SPDX-License-Identifier: MPL-2.0
use std::path::PathBuf;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchKind {
    Created,
    Modified,
    Removed,
    RenameFrom,
    RenameTo,
    RescanNeeded,
}
/// Rename halves stay ordered, including across buffers; consumers revalidate identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchEvent {
    pub directory: PathBuf,
    pub name: PathBuf,
    pub kind: WatchKind,
}
