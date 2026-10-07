// SPDX-License-Identifier: MPL-2.0
//! Command-line output goes to the standard streams of the invoking shell. The
//! arguments themselves (files to open, `--software`, `--hardware`, `--smoke`
//! and the rest) are parsed by the shell, identically on every system.
use std::io::IsTerminal;

pub fn report(text: &str, error: bool) {
    if error {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}
/// The failure was already written to standard error; repeat it in full.
pub fn show_startup_error(message: &str) {
    eprintln!("{message}");
}
pub fn stderr_redirected() -> bool {
    !std::io::stderr().is_terminal()
}
/// A terminal is never read: it would wait for typing nobody expects (APP-09).
pub fn stdin_redirected() -> bool {
    !std::io::stdin().is_terminal()
}
