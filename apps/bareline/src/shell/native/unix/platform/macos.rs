// SPDX-License-Identifier: MPL-2.0
//! macOS: the AppKit menu bar, pasteboard, panels and alerts of
//! `bareline-platform-macos`, created on the main thread (where winit runs the
//! event loop on macOS) and kept there: none of them is `Send`.
//!
//! Panels and alerts run modally (`runModal`), which is how the crate is
//! written and how AppKit applications ask: AppKit keeps the application's run
//! loop turning inside a modal session, so the window repaints and macOS never
//! marks it unresponsive, and the shell's synchronous call sites get their
//! answer directly, as on Windows. The deferred model Linux needs (see
//! `interaction`) therefore does not apply, and its seam functions do nothing
//! here.
//!
//! Menu commands chosen in the menu bar arrive as [`CommandMessage`] through
//! the sender `install_message_hook` keeps, followed by the shell's wake.
use super::super::super::prompt::PromptView;
use super::super::{
    error::{Error, Result, unsupported},
    printing,
    window::{RawWindow, command_sender, event_notify},
};
use bareline_platform::{Capability, PlatformServices, SaveDialogOptions};
use bareline_platform_macos::{MacClipboard, MacDialogs, MacMenuBar, MainThreadMarker, MenuDispatch};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub use bareline_platform_macos::{AboutAction, CommandMessage, SaveChoice, SavePromptOutcome};

/// The window a menu command was sent to, for the command trace.
pub fn command_window(message: &CommandMessage) -> RawWindow {
    message.window()
}

fn clipboard_error(error: std::io::Error) -> Error {
    Error::other(error.to_string())
}

pub struct Platform {
    menu_bar: MacMenuBar,
    clipboard: MacClipboard,
    dialogs: MacDialogs,
    clipboard_max_bytes: Cell<usize>,
    /// The path the last save panel returned. `NSSavePanel` always asks before
    /// replacing an existing file and has no public switch to leave that to the
    /// application, so replacing exactly this path is already confirmed.
    panel_confirmed: RefCell<Option<PathBuf>>,
}
impl Platform {
    /// Builds the menu bar from the shell's menu model and installs it as the
    /// application's main menu.
    ///
    /// # Safety
    /// `raw` is the seam's token for the live editor window of this thread; it
    /// only travels back in each [`CommandMessage`]. This must run on the main
    /// thread, which is checked: AppKit objects cannot be made elsewhere.
    pub unsafe fn new(
        raw: RawWindow,
        registry: &bareline_commands::CommandRegistry,
        model: bareline_commands::MenuModel,
    ) -> Result<Self> {
        let mtm = MainThreadMarker::new().ok_or_else(|| Error::other("AppKit needs the main thread"))?;
        let dispatch = MenuDispatch {
            sender: command_sender(),
            notify: event_notify(),
        };
        Ok(Self {
            menu_bar: MacMenuBar::new(mtm, raw, registry, model, dispatch),
            clipboard: MacClipboard::new(mtm),
            dialogs: MacDialogs::new(mtm),
            clipboard_max_bytes: Cell::new(bareline_platform::clipboard::DEFAULT_CLIPBOARD_MAX_BYTES),
            panel_confirmed: RefCell::new(None),
        })
    }
    /// Puts the menu target into the editor view's responder chain, so the
    /// standard Edit actions (Undo, Cut, Copy, Paste, Select All) reach the
    /// editor while its window is key.
    pub(in super::super) fn attach_window(&self, window: &Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
            // SAFETY: winit's AppKit handle names the editor window's live
            // NSView, and the shell calls this from the event loop, which runs
            // on the main thread on macOS.
            unsafe { self.menu_bar.attach_to_view(handle.ns_view) };
        }
    }
    /// A confirmation with Cancel as the default button.
    fn confirm(&self, message: &str, detail: &str, action: &str) -> bool {
        self.dialogs.confirm(message, detail, action)
    }
    /// No external program runs on these systems yet (`resolve_program`), so
    /// there is nothing to confirm.
    pub fn confirm_external_command(&self, _program: &Path, _arguments: &[std::ffi::OsString], _shell: bool) -> bool {
        false
    }
    pub fn choose_printer(
        &self,
    ) -> std::result::Result<Option<printing::PrinterSelection>, bareline_platform::printing::PrintError> {
        printing::choose_printer(None)
    }
    /// Remote reads are not offered here; nothing is read.
    pub fn confirm_remote_read(&self, _path: &Path, _action: bareline_platform::RemoteReadAction) -> bool {
        false
    }
    /// Native menus follow the system appearance.
    pub fn menu_colors(&self, _background: u32, _text: u32, _selection: u32) {}
    pub fn set_dark_mode(&self, _dark: bool) {}
    pub fn about_details(&self, details: &str) -> Result<Option<AboutAction>> {
        self.dialogs.about_details(details).map_err(Error::other)
    }
    pub fn command_id(&self, menu_id: usize) -> Option<bareline_commands::CommandId> {
        self.menu_bar.command_id(menu_id)
    }
    pub fn sync_commands_localized(
        &self,
        registry: &bareline_commands::CommandRegistry,
        context: &bareline_commands::CommandContext,
        keymap: &bareline_commands::Keymap,
        locale_revision: u64,
        label_for: impl Fn(&str, &str) -> String,
    ) -> Result<()> {
        self.menu_bar
            .sync_commands_localized(registry, context, keymap, locale_revision, label_for);
        Ok(())
    }
    pub fn refresh_structure(
        &mut self,
        registry: &bareline_commands::CommandRegistry,
        context: &bareline_commands::CommandContext,
    ) -> Result<()> {
        self.menu_bar.refresh_structure(registry, context);
        Ok(())
    }
    pub fn set_menu_item_actions(&self, actions: &[(bareline_commands::CommandId, Vec<(u16, String)>)]) {
        self.menu_bar.set_menu_item_actions(actions);
    }
    pub fn accepts_command(&self, message: &CommandMessage) -> bool {
        self.menu_bar.accepts_command(message)
    }
    #[allow(dead_code, reason = "named only by the Windows menu-command path")]
    pub fn action(&self, menu_id: usize) -> Option<bareline_commands::Action> {
        self.menu_bar.action(menu_id)
    }
    pub fn confirm_discard_and_reload(&self, name: &str) -> bool {
        self.confirm(
            &format!("Discard unsaved changes to {name} and reload it from disk?"),
            "",
            "Discard and Reload",
        )
    }
    pub fn task_dialog(
        &self,
        title: &str,
        instruction: &str,
        content: &str,
        buttons: &[(i32, &str)],
        default_button: i32,
    ) -> i32 {
        self.dialogs
            .task_dialog(title, instruction, content, buttons, default_button)
    }
    pub fn confirm_save_document(&self, name: &str) -> SavePromptOutcome {
        self.dialogs.confirm_save_document(name)
    }
    pub fn confirm_save_all(&self, names: &[String]) -> SavePromptOutcome {
        self.dialogs.confirm_save_all(names)
    }
    pub fn confirm_stop_monitoring(&self) -> bool {
        self.confirm(
            "Stop monitoring and load the current file for editing?",
            "The tab will remain open.",
            "Stop Monitoring",
        )
    }
    pub fn confirm_encoding_reinterpret(&self, name: &str, target: &str) -> bool {
        let display = |value: &str| value.chars().filter(|c| !c.is_control()).take(256).collect::<String>();
        self.confirm(
            &format!("Reopen {} using {}?", display(name), display(target)),
            "Unsaved changes in this document will be discarded. The file on disk will not be changed.",
            "Reopen",
        )
    }
    /// The person already confirmed replacing the file the save panel just
    /// returned (the panel asked); any other replacement is asked here.
    pub fn confirm_overwrite(&self, path: &Path) -> bool {
        if self
            .panel_confirmed
            .borrow()
            .as_deref()
            .is_some_and(|confirmed| confirmed == path)
        {
            self.panel_confirmed.borrow_mut().take();
            return true;
        }
        self.confirm(
            "Replace the existing file with this document?",
            &format!(
                "{}\n\nReplacing it will overwrite its current contents.",
                path.display()
            ),
            "Replace",
        )
    }
    pub fn operation_failed(&self, details: &str) {
        // The details name files and paths; the diagnostic keeps the kind only.
        eprintln!("event=operation_failed");
        self.dialogs.operation_failed(details);
    }
    pub fn clipboard_max_bytes(&self) -> usize {
        self.clipboard_max_bytes.get()
    }
    pub fn set_clipboard_max_bytes(&self, bytes: usize) {
        self.clipboard_max_bytes.set(bytes);
    }
    /// The panels keep their own recent locations.
    pub fn set_dialog_recent(&self, _enabled: bool) {}
    pub fn clipboard_text(&self) -> Result<String> {
        self.clipboard.text(self.clipboard_max_bytes()).map_err(clipboard_error)
    }
    pub fn clipboard_text_if_any(&self) -> Result<Option<String>> {
        self.clipboard
            .text_within(self.clipboard_max_bytes())
            .map_err(clipboard_error)
    }
    pub fn clipboard_text_within(&self, limit: usize) -> Result<Option<String>> {
        self.clipboard
            .text_within(limit.min(self.clipboard_max_bytes()))
            .map_err(clipboard_error)
    }
    pub fn set_clipboard_text(&self, text: &str) -> Result<()> {
        self.clipboard
            .set_text(text, self.clipboard_max_bytes())
            .map_err(clipboard_error)
    }
    pub fn set_clipboard_text_with_metadata(&self, text: &str, format: &str, bytes: &[u8]) -> Result<()> {
        self.clipboard
            .set_text_with_metadata(text, self.clipboard_max_bytes(), format, bytes)
            .map_err(clipboard_error)
    }
    pub fn clipboard_text_with_metadata(
        &self,
        format: &str,
        max_bytes: usize,
    ) -> Result<Option<bareline_platform::clipboard::ClipboardContents>> {
        self.clipboard
            .text_with_metadata(self.clipboard_max_bytes(), format, max_bytes)
            .map_err(clipboard_error)
    }
    /// Context menus through `popUpMenuPositioningItem` are a follow-up.
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
        self.dialogs.open_files()
    }
}
impl PlatformServices for Platform {
    fn about(&self) {
        self.dialogs.about();
    }
    fn open_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.dialogs.open_file()
    }
    fn save_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.save_file_with(&SaveDialogOptions::new(bareline_platform::SaveFileKind::Any))
    }
    fn save_file_with(&self, options: &SaveDialogOptions) -> std::result::Result<Option<PathBuf>, String> {
        self.panel_confirmed.borrow_mut().take();
        let chosen = PlatformServices::save_file_with(&self.dialogs, options)?;
        if let Some(path) = &chosen {
            *self.panel_confirmed.borrow_mut() = Some(path.clone());
        }
        Ok(chosen)
    }
    fn pick_folder(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.dialogs.pick_folder()
    }
}

/// Panels and alerts answer directly here; nothing is ever deferred.
pub struct Scope;
pub fn interaction_scope<R: 'static>(_platform: Option<&Platform>, _owner: impl FnOnce() -> R) -> Scope {
    Scope
}
/// A save destination check completed: the save panel's confirmation that
/// preceded it no longer applies to anything.
pub fn save_destination_settled(platform: Option<&Platform>) {
    if let Some(platform) = platform {
        platform.panel_confirmed.borrow_mut().take();
    }
}
pub fn interaction_waiting(_platform: Option<&Platform>) -> bool {
    false
}
pub fn interaction_replay<R: 'static>(_platform: Option<&Platform>) -> Option<R> {
    None
}
pub fn interaction_settle(_platform: Option<&Platform>) {}
/// Prompts are AppKit alerts here, never drawn by the shell.
pub fn in_app_prompt(_platform: Option<&Platform>) -> Option<PromptView> {
    None
}
pub fn answer_prompt(_platform: Option<&Platform>, _id: i32) {}
pub fn answer_prompt_text(_platform: Option<&Platform>, _id: i32, _text: &str) {}
/// The save panel always exists here, so the shell never needs to ask for a
/// path itself.
pub fn save_destination_prompt(_platform: Option<&Platform>, _default: &std::path::Path) -> Option<std::path::PathBuf> {
    None
}
