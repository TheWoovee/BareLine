// SPDX-License-Identifier: MPL-2.0
//! Dialogs, prompts, menus and the clipboard of the editor window (`Platform`),
//! with the answer types the Windows dialogs return. Linux and macOS differ
//! here more than anywhere else: Linux asks through the desktop portal and the
//! shell's own prompts, which answer later; macOS has a native menu bar and
//! AppKit panels and alerts, which answer at once.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;

/// What the shell says when the close (`exit` false) or exit prompt could not
/// be shown. A Windows error code means nothing here, so the words say what
/// stayed open and the way out instead (LNX-UI-007, LNX-MSG-012).
pub fn save_prompt_failed(exit: bool, _code: i32) -> String {
    if exit {
        "Bareline could not ask about the unsaved documents, so it stays open. Save them, or close them one at a time; their text stays in recovery meanwhile.".into()
    } else {
        "Bareline could not ask about the unsaved changes, so the document stays open. Save it first; its text stays in recovery meanwhile.".into()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_save_prompt_names_no_windows_error_code() {
        for exit in [false, true] {
            let message = super::save_prompt_failed(exit, -2_147_467_263);
            assert!(!message.contains("HRESULT") && !message.contains("0x8"), "{message}");
            assert!(message.contains("stays open"), "{message}");
        }
    }
}
