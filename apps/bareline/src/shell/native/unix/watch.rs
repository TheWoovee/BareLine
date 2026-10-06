// SPDX-License-Identifier: MPL-2.0
//! File watching arrives with `notify` (inotify, FSEvents) in PR-031 and PR-032.
use super::error::unsupported_io;
use bareline_platform::Capability;
use std::{io, path::PathBuf, sync::Arc};

pub struct WatchService;
// The shell drops a replaced service on its setup thread; the Windows service
// stops a worker there, so this stand-in is explicitly droppable too.
impl Drop for WatchService {
    fn drop(&mut self) {}
}
impl WatchService {
    pub fn start_notifying(_paths: Vec<PathBuf>, _notify: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        Err(unsupported_io(Capability::FileWatch))
    }
    pub fn try_recv(&self) -> Option<bareline_platform::WatchEvent> {
        None
    }
}
