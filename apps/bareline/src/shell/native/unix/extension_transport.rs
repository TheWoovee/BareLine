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
//! macOS contains it with `MacHostSandbox`: the sandbox profile applied by
//! `sandbox_init` (through `sandbox-exec`) and resource limits, proven on each
//! launch before the host starts. Where that cannot be established the launch
//! fails with "isolation=Unsupported (...)" as on Linux, and the host never
//! runs unconfined.
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
    use bareline_extensions_protocol::{BrokerResponse, Envelope};
    pub use bareline_platform_posix::extension_transport::{HostLaunch, HostLifecycle};
    use bareline_platform_posix::extension_transport::{Isolation, run_verified_host_in};
    use std::{
        io,
        sync::{Arc, OnceLock, atomic::AtomicBool},
    };

    /// The shared verified launch, contained by `MacHostSandbox`.
    pub fn run_verified_host_observed(
        launch: HostLaunch<'_>,
        cancelled: Arc<AtomicBool>,
        observe: impl FnMut(HostLifecycle),
        broker: impl FnMut(Envelope) -> BrokerResponse,
    ) -> io::Result<()> {
        run_verified_host_in(
            &bareline_platform_macos::MacHostSandbox::default(),
            launch,
            cancelled,
            observe,
            broker,
        )
        .map(|_| ())
    }
    /// How an extension host is confined here, for the Extensions page:
    /// `isolation=sandbox_init`, or why it cannot run. Nothing is started for
    /// this answer; each launch proves the profile before the host runs.
    pub fn isolation() -> Option<String> {
        static PROBED: OnceLock<String> = OnceLock::new();
        Some(
            PROBED
                .get_or_init(|| {
                    let status = match bareline_platform_macos::MacHostSandbox::isolation() {
                        isolation @ Isolation::Enforced(_) => isolation.to_string(),
                        unsupported => format!("{unsupported}; extensions do not run without it"),
                    };
                    eprintln!("event=extension_isolation status={status:?}");
                    status
                })
                .clone(),
        )
    }
}
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// The Extensions page names the host's confinement on these systems: the
    /// Landlock ABI or the macOS sandbox, or why the host will not run.
    #[test]
    fn the_host_confinement_is_named_in_plain_words() {
        let status = isolation().unwrap();
        assert!(status.starts_with("isolation="), "{status}");
        #[cfg(target_os = "linux")]
        assert!(
            status.starts_with("isolation=Landlock ABI ") || status.contains("extensions do not run without it"),
            "{status}"
        );
        #[cfg(target_os = "macos")]
        assert!(
            status == "isolation=sandbox_init" || status.contains("extensions do not run without it"),
            "{status}"
        );
    }
}
