// SPDX-License-Identifier: MPL-2.0
//! Dialogs, menus and the clipboard of the editor window (`Platform`), with the
//! answer types the Windows dialogs return. Without native dialogs every
//! question takes its safe answer: nothing is discarded, overwritten or run,
//! and a save prompt reports that it is unavailable.
use super::{
    error::{Error, NOT_IMPLEMENTED, Result, unsupported},
    printing,
    window::RawWindow,
};
use bareline_platform::{Capability, PlatformServices, SaveDialogOptions};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
use bareline_platform_linux::NativePlatform as Services;
#[cfg(target_os = "macos")]
use bareline_platform_macos::NativePlatform as Services;

/// The Windows `IDCANCEL` answer, which the shell treats as a cancelled dialog.
const DIALOG_CANCELLED: i32 = 2;

fn clipboard_unavailable() -> Error {
    Error::other("This system does not support the clipboard yet")
}

/// Answer to a "save changes?" prompt; the same shape as on Windows.
#[allow(
    dead_code,
    reason = "no save prompt exists here, so only the Windows prompt produces these answers"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
}
#[allow(
    dead_code,
    reason = "no save prompt exists here, so only the Windows prompt produces these answers"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePromptOutcome {
    Choice { choice: SaveChoice, selected: i32 },
    Failure(i32),
}
/// A native menu command; never produced here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMessage {
    /// The window the command was sent to.
    pub window: RawWindow,
    pub id: usize,
    pub action: u16,
}
/// The window a menu command was sent to, for the command trace.
pub fn command_window(message: &CommandMessage) -> RawWindow {
    message.window
}
#[allow(
    dead_code,
    reason = "the About dialog is unsupported here, so it never returns an action"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutAction {
    License,
    ThirdPartyNotices,
    CopyDiagnostics,
}

pub struct Platform {
    services: Services,
    clipboard_max_bytes: Cell<usize>,
}
impl Platform {
    /// # Safety
    /// None for this stand-in, which keeps no handle. The signature matches the
    /// Windows adapter (`raw` is the live editor window of this thread, which
    /// outlives the platform and the renderer created for it), so the shared
    /// call site stays the same.
    pub unsafe fn new(
        _raw: RawWindow,
        _registry: &bareline_commands::CommandRegistry,
        _model: bareline_commands::MenuModel,
    ) -> Result<Self> {
        Ok(Self {
            services: Services,
            clipboard_max_bytes: Cell::new(bareline_platform::clipboard::DEFAULT_CLIPBOARD_MAX_BYTES),
        })
    }
    /// A platform without a window, for unit tests.
    #[cfg(test)]
    pub(super) fn for_tests() -> Self {
        Self {
            services: Services,
            clipboard_max_bytes: Cell::new(64),
        }
    }
    pub fn confirm_external_command(&self, _program: &Path, _arguments: &[std::ffi::OsString], _shell: bool) -> bool {
        false
    }
    pub fn choose_printer(
        &self,
    ) -> std::result::Result<Option<printing::PrinterSelection>, bareline_platform::printing::PrintError> {
        printing::choose_printer(None)
    }
    pub fn confirm_remote_read(&self, _path: &Path, _action: bareline_platform::RemoteReadAction) -> bool {
        false
    }
    pub fn menu_colors(&self, _background: u32, _text: u32, _selection: u32) {}
    pub fn set_dark_mode(&self, _dark: bool) {}
    pub fn about_details(&self, _details: &str) -> Result<Option<AboutAction>> {
        Err(unsupported(Capability::About).into())
    }
    pub fn command_id(&self, _menu_id: usize) -> Option<bareline_commands::CommandId> {
        None
    }
    /// The menu bar is drawn by the shell on these systems later (ADR-C tier 2);
    /// until then there is nothing native to keep in sync.
    pub fn sync_commands_localized(
        &self,
        _registry: &bareline_commands::CommandRegistry,
        _context: &bareline_commands::CommandContext,
        _keymap: &bareline_commands::Keymap,
        _locale_revision: u64,
        _label_for: impl Fn(&str, &str) -> String,
    ) -> Result<()> {
        Ok(())
    }
    pub fn refresh_structure(
        &mut self,
        _registry: &bareline_commands::CommandRegistry,
        _context: &bareline_commands::CommandContext,
    ) -> Result<()> {
        Ok(())
    }
    pub fn set_menu_item_actions(&self, _actions: &[(bareline_commands::CommandId, Vec<(u16, String)>)]) {}
    pub fn accepts_command(&self, _message: &CommandMessage) -> bool {
        false
    }
    #[allow(dead_code, reason = "named only by the Windows menu-command path")]
    pub fn action(&self, _menu_id: usize) -> Option<bareline_commands::Action> {
        None
    }
    pub fn confirm_discard_and_reload(&self, _name: &str) -> bool {
        false
    }
    pub fn task_dialog(
        &self,
        _title: &str,
        _instruction: &str,
        _content: &str,
        _buttons: &[(i32, &str)],
        _default_button: i32,
    ) -> i32 {
        DIALOG_CANCELLED
    }
    pub fn confirm_save_document(&self, _name: &str) -> SavePromptOutcome {
        SavePromptOutcome::Failure(NOT_IMPLEMENTED)
    }
    pub fn confirm_save_all(&self, _names: &[String]) -> SavePromptOutcome {
        SavePromptOutcome::Failure(NOT_IMPLEMENTED)
    }
    pub fn confirm_stop_monitoring(&self) -> bool {
        false
    }
    pub fn confirm_encoding_reinterpret(&self, _name: &str, _target: &str) -> bool {
        false
    }
    pub fn confirm_overwrite(&self, _path: &Path) -> bool {
        false
    }
    /// No native message box: the failure goes to the diagnostic output.
    pub fn operation_failed(&self, details: &str) {
        eprintln!("event=operation_failed details={details}");
    }
    pub fn clipboard_max_bytes(&self) -> usize {
        self.clipboard_max_bytes.get()
    }
    pub fn set_clipboard_max_bytes(&self, bytes: usize) {
        self.clipboard_max_bytes.set(bytes);
    }
    pub fn set_dialog_recent(&self, _enabled: bool) {}
    pub fn clipboard_text(&self) -> Result<String> {
        Err(clipboard_unavailable())
    }
    pub fn clipboard_text_if_any(&self) -> Result<Option<String>> {
        Err(clipboard_unavailable())
    }
    pub fn clipboard_text_within(&self, _limit: usize) -> Result<Option<String>> {
        Err(clipboard_unavailable())
    }
    pub fn set_clipboard_text(&self, _text: &str) -> Result<()> {
        Err(clipboard_unavailable())
    }
    pub fn set_clipboard_text_with_metadata(&self, _text: &str, _format: &str, _bytes: &[u8]) -> Result<()> {
        Err(clipboard_unavailable())
    }
    pub fn clipboard_text_with_metadata(
        &self,
        _format: &str,
        _max_bytes: usize,
    ) -> Result<Option<bareline_platform::clipboard::ClipboardContents>> {
        Err(clipboard_unavailable())
    }
    pub fn context_menu_in(
        &self,
        _x: i32,
        _y: i32,
        _registry: &bareline_commands::CommandRegistry,
        _context: &bareline_commands::CommandContext,
        _keymap: &bareline_commands::Keymap,
        _commands: &[bareline_commands::CommandId],
    ) -> Result<Option<bareline_commands::Action>> {
        Err(unsupported(Capability::ContextMenu).into())
    }
    pub fn open_files(&self) -> std::result::Result<Vec<PathBuf>, String> {
        Err(unsupported(Capability::OpenFile).to_string())
    }
}
impl PlatformServices for Platform {
    fn about(&self) {
        self.services.about();
    }
    fn open_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.services.open_file()
    }
    fn save_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.services.save_file()
    }
    fn save_file_with(&self, options: &SaveDialogOptions) -> std::result::Result<Option<PathBuf>, String> {
        self.services.save_file_with(options)
    }
    fn pick_folder(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.services.pick_folder()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_take_their_safe_answer_and_features_answer_in_plain_language() {
        let platform = Platform::for_tests();
        let refusal = "This system does not support";
        assert!(
            platform
                .clipboard_text_within(64)
                .unwrap_err()
                .message()
                .starts_with(refusal)
        );
        assert!(
            platform
                .set_clipboard_text("text")
                .unwrap_err()
                .to_string()
                .starts_with(refusal)
        );
        assert!(platform.open_files().unwrap_err().starts_with(refusal));
        assert!(platform.open_file().unwrap_err().starts_with(refusal));
        assert!(
            platform
                .choose_printer()
                .err()
                .is_some_and(|error| error.to_string().starts_with(refusal))
        );
        // Nothing is overwritten, discarded or closed.
        assert!(!platform.confirm_overwrite(Path::new("/tmp/kept.txt")));
        assert!(!platform.confirm_discard_and_reload("kept.txt"));
        assert_eq!(
            platform.confirm_save_document("kept.txt"),
            SavePromptOutcome::Failure(NOT_IMPLEMENTED)
        );
        assert_eq!(platform.task_dialog("t", "i", "c", &[(1, "OK")], 1), DIALOG_CANCELLED);
        let message = CommandMessage {
            window: 7,
            id: 1,
            action: 0,
        };
        assert_eq!(command_window(&message), 7);
        assert!(!platform.accepts_command(&message));
    }
}
