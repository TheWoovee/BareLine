// SPDX-License-Identifier: MPL-2.0
//! Single-instance handoff (a Unix socket lock, ADR-C) is not built yet:
//! every window runs on its own and leaves the shared session alone.
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenRequest {
    pub paths: Vec<PathBuf>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub read_only: bool,
    pub monitor: bool,
}
#[allow(
    dead_code,
    reason = "every window runs independently until the instance handoff exists"
)]
pub enum Outcome {
    Forwarded,
    Primary(InstanceServer),
    Independent(String),
}
pub struct InstanceServer;
impl InstanceServer {
    pub fn try_recv(&self) -> Option<OpenRequest> {
        None
    }
    pub fn set_accepting(&self, _accepting: bool) {}
    #[allow(dead_code, reason = "named only by the Windows handoff path")]
    pub fn pending(&self) -> usize {
        0
    }
    pub fn quiesce(&self) -> usize {
        0
    }
}
pub fn coordinate(
    _scope: &Path,
    _profile: Option<&Path>,
    _request: OpenRequest,
    _new_instance: bool,
    _notify: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<Outcome> {
    Ok(Outcome::Independent(
        "This system does not support handing files to a running Bareline window yet, so this window runs separately; its tabs are not restored next time.".into(),
    ))
}
