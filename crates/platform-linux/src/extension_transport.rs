// SPDX-License-Identifier: MPL-2.0
//! The extension host launch on Linux: the shared Unix transport and verified
//! launch of `bareline-platform-posix`, contained by [`LinuxHostSandbox`]. The
//! names match the Windows adapter's module, so the shell's seam can re-export
//! this module as it is.
use crate::sandbox::LinuxHostSandbox;
use bareline_extensions_protocol::{BrokerResponse, Envelope};
pub use bareline_platform_posix::extension_transport::{
    AuthenticatedSocket, HostLaunch, HostLifecycle, Isolation, UnixTransportServer, run_verified_host_in,
};
use std::{
    io,
    sync::{Arc, atomic::AtomicBool},
};

/// Synchronous worker entry, as on Windows. The host runs only where Landlock
/// confines it; elsewhere this fails with `Unsupported` and the launch is not
/// attempted (SEC-05).
pub fn run_verified_host(
    launch: HostLaunch<'_>,
    cancelled: Arc<AtomicBool>,
    broker: impl FnMut(Envelope) -> BrokerResponse,
) -> io::Result<()> {
    run_verified_host_observed(launch, cancelled, |_| {}, broker)
}
pub fn run_verified_host_observed(
    launch: HostLaunch<'_>,
    cancelled: Arc<AtomicBool>,
    observe: impl FnMut(HostLifecycle),
    broker: impl FnMut(Envelope) -> BrokerResponse,
) -> io::Result<()> {
    run_verified_host_in(&LinuxHostSandbox::default(), launch, cancelled, observe, broker).map(|_| ())
}
