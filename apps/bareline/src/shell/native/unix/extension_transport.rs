// SPDX-License-Identifier: MPL-2.0
//! The extension host transport (a Unix domain socket with peer credentials,
//! ADR-C) is not built yet; no extension host is started.
use bareline_extensions_protocol::{BrokerResponse, Envelope, ExecutionBudget, Invocation};
use std::{
    io,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[allow(
    dead_code,
    reason = "no extension host runs here, so the launch is never read or observed"
)]
pub struct HostLaunch<'a> {
    pub executable: &'a Path,
    pub executable_sha256: [u8; 32],
    pub signer: &'a bareline_distribution::update::PublisherPin,
    pub component: &'a Path,
    pub component_sha256: [u8; 32],
    pub invocation: &'a Invocation,
    pub budget: ExecutionBudget,
}
#[allow(
    dead_code,
    reason = "no extension host runs here, so the launch is never read or observed"
)]
#[derive(Clone, Copy, Debug)]
pub enum HostLifecycle {
    Started(u32),
    Authenticated(u32),
    Drained(u32),
}
pub fn run_verified_host_observed(
    _launch: HostLaunch<'_>,
    _cancelled: Arc<AtomicBool>,
    _observe: impl FnMut(HostLifecycle),
    _broker: impl FnMut(Envelope) -> BrokerResponse,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This system does not support running extensions yet",
    ))
}
