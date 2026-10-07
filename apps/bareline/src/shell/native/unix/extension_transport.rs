// SPDX-License-Identifier: MPL-2.0
//! The extension host transport: an authenticated Unix socket (peer pid and
//! uid checked on both ends) and the verified launch of the host, as on
//! Windows.
//!
//! Linux contains the host with `LinuxHostSandbox` (Landlock, resource limits,
//! no new privileges, a scrubbed environment). Like Windows without its job and
//! AppContainer, a kernel without Landlock does not run the host at all: the
//! launch fails with "isolation=Unsupported (...)", which the shell shows as the
//! extension's error (SEC-05), and [`isolation`] names the state for the
//! Extensions page.
//!
//! macOS: `MacIsolation` (sandbox-exec and limits) spawns through
//! `posix_spawn`, which the shared launch's `HostSandbox` (a
//! `std::process::Child`) cannot carry yet, so no host is started there.
#[cfg(target_os = "linux")]
mod linux {
    pub use bareline_platform_linux::extension_transport::{HostLaunch, HostLifecycle, run_verified_host_observed};
    use std::sync::OnceLock;

    /// How an extension host would be confined on this system, for the
    /// Extensions page: `isolation=Landlock ABI <n>`, or why it cannot run.
    pub fn isolation() -> Option<String> {
        static PROBED: OnceLock<String> = OnceLock::new();
        Some(
            PROBED
                .get_or_init(|| {
                    let status = match bareline_platform_linux::sandbox::probe() {
                        isolation @ bareline_platform_linux::extension_transport::Isolation::Enforced(_) => {
                            isolation.to_string()
                        }
                        unsupported => format!("{unsupported}; extensions do not run without it"),
                    };
                    eprintln!("event=extension_isolation status={status:?}");
                    status
                })
                .clone(),
        )
    }
}
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos {
    use bareline_extensions_protocol::{BrokerResponse, Envelope, ExecutionBudget, Invocation};
    use std::{
        io,
        path::Path,
        sync::{Arc, atomic::AtomicBool},
    };

    #[allow(
        dead_code,
        reason = "no extension host runs on macOS yet, so the launch is never read"
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
        reason = "no extension host runs on macOS yet, so the launch is never observed"
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
        Err(io::Error::new(io::ErrorKind::Unsupported, unsupported()))
    }
    fn unsupported() -> String {
        "isolation=unsupported (the macOS sandbox does not start the extension host yet)".into()
    }
    pub fn isolation() -> Option<String> {
        Some(unsupported())
    }
}
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// The Extensions page names the host's confinement on these systems: the
    /// Landlock ABI, or why the host will not run (macOS, or a kernel without
    /// Landlock).
    #[test]
    fn the_host_confinement_is_named_in_plain_words() {
        let status = isolation().unwrap();
        assert!(status.starts_with("isolation="), "{status}");
        #[cfg(target_os = "linux")]
        assert!(
            status.starts_with("isolation=Landlock ABI ") || status.contains("extensions do not run without it"),
            "{status}"
        );
    }
}
