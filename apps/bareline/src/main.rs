// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#[cfg(windows)]
mod windows_app;
fn main() {
    bareline_diagnostics::install_panic_hook();
    #[cfg(windows)]
    if let Err(error) = windows_app::run() {
        eprintln!("event=startup_failed error={error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!(
        "The native shell currently supports Windows. Neutral crates support headless verification."
    );
}
