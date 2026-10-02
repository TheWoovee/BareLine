// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// ARC-01: no function in the shell may grow past the clippy.toml threshold.
// The functions already above it carry an explicit
// `#[allow(clippy::too_many_lines)]`, so any new one is reported.
#![warn(clippy::too_many_lines)]
#[cfg(feature = "configured-release")]
const _: () = assert!(
    !bareline_file_io::QA_FAULTS_ENABLED,
    "qa-faults is diagnostic-only and cannot be linked into a shipping configured release"
);
#[cfg(feature = "qa-faults")]
mod qa_faults;
#[cfg(windows)]
mod windows_app;
mod build_capabilities {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/capability_assertion.rs"
    ));
}
/// How long a fatal panic may wait for queued recovery checkpoints and journal
/// appends to become durable before the process aborts.
const PANIC_SEAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
fn main() {
    // Release builds use `panic = "abort"`; this hook is the only code that runs
    // after a panic. It seals recovery work, writes a text-free crash record, then aborts.
    bareline_diagnostics::install_fatal_panic_hook(bareline_file_io::recovery_seal::seal, PANIC_SEAL_BUDGET);
    build_capabilities::retain();
    #[cfg(feature = "qa-faults")]
    bareline_file_io::install_qa_save_boundary_hook(qa_faults::hit)
        .expect("QA save boundary hook must be installed exactly once");
    #[cfg(windows)]
    if let Err(error) = windows_app::run() {
        eprintln!("event=startup_failed error={error}");
        windows_app::report_startup_failure(&*error);
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!("The native shell currently supports Windows. Neutral crates support headless verification.");
}
