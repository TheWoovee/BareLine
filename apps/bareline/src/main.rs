// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
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
fn main() {
    build_capabilities::retain();
    #[cfg(feature = "qa-faults")]
    bareline_file_io::install_qa_save_boundary_hook(qa_faults::hit)
        .expect("QA save boundary hook must be installed exactly once");
    bareline_diagnostics::install_panic_hook();
    #[cfg(windows)]
    if let Err(error) = windows_app::run() {
        eprintln!("event=startup_failed error={error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!("The native shell currently supports Windows. Neutral crates support headless verification.");
}
