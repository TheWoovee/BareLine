// SPDX-License-Identifier: MPL-2.0
//! Linux: the portal's file dialogs and the shell's own prompts, which both
//! answer later (see `interaction`), and the X11 or Wayland clipboard. There is
//! no native menu bar (the shell draws its menus, ADR-C tier 2), so no menu
//! command ever arrives.
//!
//! Clipboard limitation: a Wayland session whose compositor offers no data
//! control protocol (ext- or wlr-data-control) and that runs no XWayland
//! (`DISPLAY` unset) leaves the editor without a clipboard. Copy and Paste then
//! fail with "This system does not support the clipboard ..." and nothing is
//! copied. Reading the Wayland clipboard without data control needs the
//! editor's own `wl_data_device` on its winit surface, which winit does not
//! expose; it is a follow-up. The desktops without data control (GNOME, WSLg)
//! normally run XWayland with `DISPLAY` set, so the X11 fallback covers them.
use super::super::super::prompt::PromptView;
use super::super::{
    error::{Error, Result, unsupported},
    interaction::{Answer, Interactions, Question, Scope},
    printing,
    window::{RawWindow, event_notify},
};
use bareline_platform::{Capability, PlatformServices, SaveDialogOptions, SaveFileKind};
use bareline_platform_linux::{InAppPrompt, LinuxClipboard, LinuxDialogs, dialogs::DialogRequest, prompts};
use std::{
    cell::{Cell, OnceCell, RefCell},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub use bareline_platform_linux::{AboutAction, SaveChoice, SavePromptOutcome};

/// The Windows `IDCANCEL` answer, which the shell treats as a cancelled dialog.
const DIALOG_CANCELLED: i32 = prompts::CANCEL_ID;

/// A native menu command; Linux has no native menu bar, so none is produced.
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

/// The session's clipboard: Wayland data control where the compositor offers
/// it, otherwise the X11 `CLIPBOARD` (which XWayland bridges to Wayland
/// clients on GNOME, Weston and WSLg, so text and Bareline's metadata still
/// travel). The fallback is logged once per process.
fn open_clipboard() -> std::result::Result<LinuxClipboard, String> {
    static DEGRADED: AtomicBool = AtomicBool::new(false);
    let named = |variable: &str| std::env::var_os(variable).is_some_and(|value| !value.is_empty());
    if named("WAYLAND_DISPLAY") {
        match LinuxClipboard::wayland() {
            Ok(clipboard) => return Ok(clipboard),
            Err(error) if named("DISPLAY") => {
                if !DEGRADED.swap(true, Ordering::Relaxed) {
                    eprintln!("event=clipboard_degraded from=wayland-data-control to=x11-xwayland reason=\"{error}\"");
                }
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if named("DISPLAY") {
        return LinuxClipboard::x11().map_err(|error| error.to_string());
    }
    Err("This system does not support the clipboard without a Wayland or X11 display".into())
}
fn no_text() -> Error {
    Error::other("The clipboard does not contain text.")
}

pub struct Platform {
    interactions: Interactions,
    clipboard: OnceCell<std::result::Result<LinuxClipboard, String>>,
    clipboard_max_bytes: Cell<usize>,
    /// The path the last Save dialog returned. The portal always asks before
    /// that dialog replaces an existing file and cannot leave the question to
    /// the application, so replacing exactly this path is already confirmed,
    /// by the save destination check that follows the dialog only: the check
    /// clears it when it completes, whatever its outcome
    /// (`save_destination_settled`), and the next dialog replaces it.
    portal_confirmed: RefCell<Option<PathBuf>>,
}
impl Platform {
    /// # Safety
    /// None here: no native handle is kept. The signature matches the Windows
    /// adapter (`raw` is the live editor window of this thread, which outlives
    /// the platform and the renderer created for it), so the shared call site
    /// stays the same.
    pub unsafe fn new(
        _raw: RawWindow,
        _registry: &bareline_commands::CommandRegistry,
        _model: bareline_commands::MenuModel,
    ) -> Result<Self> {
        Ok(Self::with_dialogs(LinuxDialogs::new()))
    }
    fn with_dialogs(dialogs: LinuxDialogs) -> Self {
        Self {
            interactions: Interactions::new(dialogs, event_notify()),
            clipboard: OnceCell::new(),
            clipboard_max_bytes: Cell::new(bareline_platform::clipboard::DEFAULT_CLIPBOARD_MAX_BYTES),
            portal_confirmed: RefCell::new(None),
        }
    }
    /// A platform without a window, a portal or a clipboard, for unit tests.
    #[cfg(test)]
    pub(in crate::shell) fn for_tests() -> Self {
        Self::for_tests_with(std::sync::Arc::new(tests::NoPortal))
    }
    /// As `for_tests`, with a portal whose dialogs `chooser` answers.
    #[cfg(test)]
    pub(in crate::shell) fn for_tests_with(
        chooser: std::sync::Arc<dyn bareline_platform_linux::portal::FileChooser>,
    ) -> Self {
        let platform = Self::with_dialogs(LinuxDialogs::with_chooser(chooser));
        platform.clipboard_max_bytes.set(64);
        let _ = platform
            .clipboard
            .set(Err("This system does not support the clipboard in unit tests".into()));
        platform
    }
    /// Attaches the portal's dialogs to the editor window on X11. A Wayland
    /// window has no handle the portal accepts without xdg-foreign, so the
    /// dialogs open unattached there.
    pub(in super::super) fn attach_window(&self, window: &Window) {
        if let Ok(handle) = window.window_handle() {
            // The portal's `x11:<hex xid>` parent (`portal::x11_parent`); Xlib
            // window ids are `c_ulong`, whose width differs by target.
            let parent = match handle.as_raw() {
                RawWindowHandle::Xlib(handle) => Some(format!("x11:{:x}", handle.window)),
                RawWindowHandle::Xcb(handle) => Some(bareline_platform_linux::portal::x11_parent(u64::from(
                    handle.window.get(),
                ))),
                _ => None,
            };
            if let Some(parent) = parent {
                self.interactions.set_parent(parent);
            }
        }
    }
    fn button(&self, prompt: InAppPrompt) -> Option<i32> {
        self.interactions
            .ask(Question::Prompt(prompt))
            .and_then(|answer| answer.button())
    }
    fn dialog(&self, request: DialogRequest) -> std::result::Result<Vec<PathBuf>, String> {
        self.interactions
            .ask(Question::Dialog(request))
            .and_then(Answer::paths)
            .unwrap_or(Ok(Vec::new()))
    }
    fn clipboard(&self) -> Result<&LinuxClipboard> {
        self.clipboard
            .get_or_init(open_clipboard)
            .as_ref()
            .map_err(|error| Error::other(error.clone()))
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
    pub fn menu_colors(&self, _background: u32, _text: u32, _selection: u32) {}
    pub fn set_dark_mode(&self, _dark: bool) {}
    pub fn about_details(&self, details: &str) -> Result<Option<AboutAction>> {
        let prompt = InAppPrompt::about(details).map_err(Error::other)?;
        Ok(self.button(prompt).and_then(prompts::about_action))
    }
    pub fn command_id(&self, _menu_id: usize) -> Option<bareline_commands::CommandId> {
        None
    }
    /// The menu bar is drawn by the shell on Linux later (ADR-C tier 2); until
    /// then there is nothing native to keep in sync.
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
    pub fn confirm_discard_and_reload(&self, name: &str) -> bool {
        self.button(InAppPrompt::discard_and_reload(name))
            .is_some_and(prompts::confirmed)
    }
    pub fn task_dialog(
        &self,
        title: &str,
        instruction: &str,
        content: &str,
        buttons: &[(i32, &str)],
        default_button: i32,
    ) -> i32 {
        self.button(InAppPrompt::task(title, instruction, content, buttons, default_button))
            .unwrap_or(DIALOG_CANCELLED)
    }
    /// Until the person answers, Cancel: nothing is saved, discarded or closed,
    /// and the shell keeps the close queued while the prompt is open.
    pub fn confirm_save_document(&self, name: &str) -> SavePromptOutcome {
        prompts::save_outcome(
            self.button(InAppPrompt::save_document(name))
                .unwrap_or(DIALOG_CANCELLED),
        )
    }
    pub fn confirm_save_all(&self, names: &[String]) -> SavePromptOutcome {
        prompts::save_outcome(self.button(InAppPrompt::save_all(names)).unwrap_or(DIALOG_CANCELLED))
    }
    pub fn confirm_stop_monitoring(&self) -> bool {
        self.button(InAppPrompt::stop_monitoring())
            .is_some_and(prompts::confirmed)
    }
    pub fn confirm_encoding_reinterpret(&self, name: &str, target: &str) -> bool {
        self.button(InAppPrompt::encoding_reinterpret(name, target))
            .is_some_and(prompts::confirmed)
    }
    /// The person already confirmed replacing the file the Save dialog just
    /// returned (the portal asked); any other replacement is asked here.
    pub fn confirm_overwrite(&self, path: &Path) -> bool {
        if self
            .portal_confirmed
            .borrow()
            .as_deref()
            .is_some_and(|confirmed| confirmed == path)
        {
            self.portal_confirmed.borrow_mut().take();
            eprintln!("event=overwrite_confirmed source=save-dialog");
            return true;
        }
        self.button(InAppPrompt::overwrite(path))
            .is_some_and(prompts::overwrite_confirmed)
    }
    /// The failure goes to the diagnostic output and, when no other question is
    /// open, into the shell's message prompt.
    pub fn operation_failed(&self, details: &str) {
        // The details name files and paths; the diagnostic keeps the kind only.
        eprintln!("event=operation_failed");
        self.interactions.notice(InAppPrompt::operation_failed(details));
    }
    pub fn clipboard_max_bytes(&self) -> usize {
        self.clipboard_max_bytes.get()
    }
    pub fn set_clipboard_max_bytes(&self, bytes: usize) {
        self.clipboard_max_bytes.set(bytes);
    }
    /// The portal keeps its own recent locations.
    pub fn set_dialog_recent(&self, _enabled: bool) {}
    pub fn clipboard_text(&self) -> Result<String> {
        self.clipboard_text_if_any()?.ok_or_else(no_text)
    }
    pub fn clipboard_text_if_any(&self) -> Result<Option<String>> {
        self.clipboard()?
            .read(self.clipboard_max_bytes())
            .map_err(|error| Error::other(error.to_string()))
    }
    /// Like `clipboard_text_if_any`, but text over `limit` bytes fails before it
    /// is decoded, so small fields never pay for a huge clipboard.
    pub fn clipboard_text_within(&self, limit: usize) -> Result<Option<String>> {
        self.clipboard()?
            .read(limit.min(self.clipboard_max_bytes()))
            .map_err(|error| Error::other(error.to_string()))
    }
    pub fn set_clipboard_text(&self, text: &str) -> Result<()> {
        self.clipboard()?
            .write(text, self.clipboard_max_bytes())
            .map_err(|error| Error::other(error.to_string()))
    }
    pub fn set_clipboard_text_with_metadata(&self, text: &str, format: &str, bytes: &[u8]) -> Result<()> {
        self.clipboard()?
            .write_with_metadata(text, self.clipboard_max_bytes(), format, bytes)
            .map_err(|error| Error::other(error.to_string()))
    }
    pub fn clipboard_text_with_metadata(
        &self,
        format: &str,
        max_bytes: usize,
    ) -> Result<Option<bareline_platform::clipboard::ClipboardContents>> {
        self.clipboard()?
            .read_with_metadata(self.clipboard_max_bytes(), format, max_bytes)
            .map_err(|error| Error::other(error.to_string()))
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
        self.dialog(DialogRequest::Open { multiple: true })
    }
}
impl PlatformServices for Platform {
    /// The shell shows About through `about_details`.
    fn about(&self) {}
    fn open_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.dialog(DialogRequest::Open { multiple: false })
            .map(|paths| paths.into_iter().next())
    }
    fn save_file(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.save_file_with(&SaveDialogOptions::new(SaveFileKind::Any))
    }
    /// The portal confirms replacing an existing file itself. Its typed name is
    /// therefore extended with the default extension only when nothing exists
    /// under the extended name, so every path returned here was either
    /// confirmed or free when chosen (see `confirm_overwrite`).
    fn save_file_with(&self, options: &SaveDialogOptions) -> std::result::Result<Option<PathBuf>, String> {
        let mut options = options.clone();
        options.app_confirms_overwrite = false;
        self.portal_confirmed.borrow_mut().take();
        let chosen = self
            .dialog(DialogRequest::Save(options))
            .map(|paths| paths.into_iter().next())?;
        if let Some(path) = &chosen {
            *self.portal_confirmed.borrow_mut() = Some(path.clone());
        }
        Ok(chosen)
    }
    fn pick_folder(&self) -> std::result::Result<Option<PathBuf>, String> {
        self.dialog(DialogRequest::PickFolder)
            .map(|paths| paths.into_iter().next())
    }
}

/// See `interaction`: the shell marks code it can run again once a prompt or
/// dialog started inside it has its answer.
pub fn interaction_scope<R: 'static>(platform: Option<&Platform>, owner: impl FnOnce() -> R) -> Scope {
    platform.map_or_else(Scope::inert, |platform| platform.interactions.scope(Box::new(owner())))
}
/// A save destination check completed: the Save dialog's confirmation that
/// preceded it no longer applies to anything.
pub fn save_destination_settled(platform: Option<&Platform>) {
    if let Some(platform) = platform {
        platform.portal_confirmed.borrow_mut().take();
    }
}
/// Whether a prompt or dialog is open, or its answer waits for its run.
pub fn interaction_waiting(platform: Option<&Platform>) -> bool {
    platform.is_some_and(|platform| platform.interactions.waiting())
}
/// The scope to run again now that its answers are armed.
pub fn interaction_replay<R: 'static>(platform: Option<&Platform>) -> Option<R> {
    let owner = platform?.interactions.take_ready()?;
    owner.downcast::<R>().ok().map(|owner| *owner)
}
/// Drops answers the last run did not consume.
pub fn interaction_settle(platform: Option<&Platform>) {
    if let Some(platform) = platform {
        platform.interactions.settle();
    }
}
/// The prompt the shell draws now, if any.
pub fn in_app_prompt(platform: Option<&Platform>) -> Option<PromptView> {
    platform?.interactions.view()
}
/// The button the person pressed in the shell's prompt.
pub fn answer_prompt(platform: Option<&Platform>, id: i32) {
    if let Some(platform) = platform {
        platform.interactions.answer(id);
    }
}
/// The button pressed in a prompt with a text field, and what the field held.
pub fn answer_prompt_text(platform: Option<&Platform>, id: i32, text: &str) {
    if let Some(platform) = platform {
        platform.interactions.answer_text(id, text);
    }
}
/// Where to save when the save dialog is unsupported (no desktop portal, as on
/// bare X11 or WSL): the shell's own prompt with a path field that starts at
/// `default` (LNX-EDIT-002). Answers later, like every prompt here: `None`
/// until then, and when the person cancels.
pub fn save_destination_prompt(platform: Option<&Platform>, default: &Path) -> Option<PathBuf> {
    platform?
        .interactions
        .ask(Question::Destination(default.to_path_buf()))
        .and_then(Answer::paths)
        .and_then(std::result::Result::ok)
        .and_then(|paths| paths.into_iter().next())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use bareline_platform_linux::portal::{ChooserMethod, ChooserOptions, ChooserResponse, FileChooser, PortalError};

    /// A session without a desktop portal (WSL, a bare X server).
    pub(in super::super) struct NoPortal;
    impl FileChooser for NoPortal {
        fn choose(
            &self,
            _method: ChooserMethod,
            _parent: &str,
            _title: &str,
            _options: &ChooserOptions,
        ) -> std::result::Result<ChooserResponse, PortalError> {
            Err(PortalError::Unavailable("no portal in unit tests".into()))
        }
    }

    fn replayed(platform: &Platform) -> Option<&'static str> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(owner) = interaction_replay::<&'static str>(Some(platform)) {
                return Some(owner);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn questions_take_their_safe_answer_until_answered_and_features_answer_in_plain_language() {
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
        assert!(
            platform
                .choose_printer()
                .err()
                .is_some_and(|error| error.to_string().starts_with(refusal))
        );
        // Outside a scope nothing is shown, and nothing is overwritten,
        // discarded, opened or closed.
        assert_eq!(platform.open_file(), Ok(None));
        assert!(!platform.confirm_overwrite(Path::new("/tmp/kept.txt")));
        assert!(!platform.confirm_discard_and_reload("kept.txt"));
        assert_eq!(
            platform.confirm_save_document("kept.txt"),
            SavePromptOutcome::Choice {
                choice: SaveChoice::Cancel,
                selected: DIALOG_CANCELLED
            }
        );
        assert_eq!(platform.task_dialog("t", "i", "c", &[(1, "OK")], 1), DIALOG_CANCELLED);
        assert!(in_app_prompt(Some(&platform)).is_none());
        let message = CommandMessage {
            window: 7,
            id: 1,
            action: 0,
        };
        assert_eq!(command_window(&message), 7);
        assert!(!platform.accepts_command(&message));
    }

    #[test]
    fn a_close_prompt_answers_dont_save_on_the_second_run() {
        let platform = Platform::for_tests();
        {
            let _scope = interaction_scope(Some(&platform), || "close");
            assert_eq!(
                platform.confirm_save_document("notes.txt \u{2022}"),
                SavePromptOutcome::Choice {
                    choice: SaveChoice::Cancel,
                    selected: DIALOG_CANCELLED
                }
            );
        }
        assert!(interaction_waiting(Some(&platform)));
        let view = in_app_prompt(Some(&platform)).expect("the prompt is drawn by the shell");
        assert_eq!(view.instruction, "Save changes to notes.txt?");
        let labels: Vec<_> = view.buttons.iter().map(|button| button.label.as_str()).collect();
        assert_eq!(labels, ["Save", "Don't Save", "Cancel"]);
        let dont_save = view.buttons[1].id;
        answer_prompt(Some(&platform), dont_save);
        assert_eq!(replayed(&platform), Some("close"));
        let _scope = interaction_scope(Some(&platform), || "close");
        assert_eq!(
            platform.confirm_save_document("notes.txt \u{2022}"),
            SavePromptOutcome::Choice {
                choice: SaveChoice::DontSave,
                selected: dont_save
            }
        );
    }

    #[test]
    fn without_a_portal_the_dialog_reports_unsupported_rather_than_waiting() {
        let platform = Platform::for_tests();
        {
            let _scope = interaction_scope(Some(&platform), || "open");
            assert_eq!(platform.open_files(), Ok(Vec::new()));
        }
        assert_eq!(replayed(&platform), Some("open"));
        let _scope = interaction_scope(Some(&platform), || "open");
        let message = platform.open_files().unwrap_err();
        assert_eq!(
            message,
            bareline_platform::Unsupported {
                capability: Capability::OpenFile
            }
            .to_string()
        );
    }

    /// The seam's clipboard functions (what Copy and Paste call) against the
    /// session's real clipboard. Needs a desktop session, so it is ignored by
    /// default; under WSLg (Wayland without data control, XWayland as X11):
    /// `cargo test -p bareline --bin bareline session_clipboard -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a Wayland or X11 session"]
    fn the_session_clipboard_copies_pastes_and_carries_metadata() {
        use bareline_platform::clipboard::RECTANGLE_CLIPBOARD_FORMAT;
        let platform = Platform::with_dialogs(LinuxDialogs::with_chooser(std::sync::Arc::new(NoPortal)));
        let backend = platform.clipboard().map(LinuxClipboard::backend);
        eprintln!(
            "event=session_clipboard WAYLAND_DISPLAY={:?} DISPLAY={:?} backend={backend:?}",
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("DISPLAY")
        );
        // Copy a line, paste it twice where it was selected: it is duplicated.
        platform
            .set_clipboard_text(
                "alpha line
",
            )
            .unwrap();
        let pasted = format!(
            "{}{}",
            platform.clipboard_text().unwrap(),
            platform.clipboard_text().unwrap()
        );
        eprintln!("event=session_clipboard pasted={pasted:?}");
        assert_eq!(
            pasted,
            "alpha line
alpha line
"
        );
        // Other applications see the copy through the session's clipboard.
        for (tool, arguments) in [
            ("wl-paste", &["--no-newline"][..]),
            ("xclip", &["-o", "-selection", "clipboard"][..]),
        ] {
            let output = std::process::Command::new(tool).args(arguments).output();
            eprintln!(
                "event=session_clipboard reader={tool} text={:?}",
                output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            );
        }
        // The metadata channel travels beside the text.
        platform
            .set_clipboard_text_with_metadata(
                "ab
cd",
                RECTANGLE_CLIPBOARD_FORMAT,
                b"rectangle 2x2",
            )
            .unwrap();
        let contents = platform
            .clipboard_text_with_metadata(RECTANGLE_CLIPBOARD_FORMAT, 64)
            .unwrap()
            .unwrap();
        eprintln!(
            "event=session_clipboard metadata={:?} text={:?}",
            contents.metadata.as_deref().map(String::from_utf8_lossy),
            contents.text
        );
        assert_eq!(
            contents.text,
            "ab
cd"
        );
        assert_eq!(contents.metadata.as_deref(), Some(&b"rectangle 2x2"[..]));
        // A second clipboard in this process does not log the fallback again.
        let again = Platform::with_dialogs(LinuxDialogs::with_chooser(std::sync::Arc::new(NoPortal)));
        assert!(again.clipboard_text_if_any().is_ok());
    }

    /// Copy and Paste through the editor's own command path (what Ctrl+C and
    /// Ctrl+V dispatch: `power_action` with the active document) against the
    /// session's real clipboard: a selected line copied and pasted twice is
    /// duplicated. Wayland windows cannot be driven by xdotool, and WSLg offers
    /// no virtual keyboard, so this is how the Wayland session's keystroke path
    /// is exercised there. Needs a desktop session, so it is ignored by default:
    /// `cargo test -p bareline --bin bareline command_path_clipboard -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a Wayland or X11 session"]
    fn the_command_path_clipboard_duplicates_a_copied_line() {
        use crate::shell::accessibility::tests::headless_shell;
        use bareline_app::workspace::{Input, Workspace};
        use bareline_commands::Action;
        use bareline_document::TextOffset;
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        fn settle(workspace: &mut Workspace, index: usize) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.editors[index].busy() {
                    return;
                }
                assert!(Instant::now() < deadline, "the edit never landed");
                std::thread::yield_now();
            }
        }
        fn text(shell: &crate::shell::Shell, index: usize) -> String {
            let snapshot = shell.workspace.as_ref().unwrap().editors[index].snapshot();
            snapshot.read(TextOffset(0)..TextOffset(snapshot.len()), 4096).unwrap()
        }
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(crate::shell::native::FileSystem)).unwrap();
        let index = workspace.new_document_with_text("alpha line\nbravo\n".into()).unwrap();
        // Select the first line with its line break, as Ctrl+Home, Shift+Down
        // do (by offset: line navigation waits for a renderer to lay out).
        workspace.editors[index].enqueue(Input::SetCaret(0, false));
        workspace.editors[index].enqueue(Input::SetCaret("alpha line\n".len(), true));
        settle(&mut workspace, index);
        let mut shell = headless_shell();
        shell.app.tabs = workspace.titles();
        shell.app.active = index;
        shell.workspace = Some(workspace);
        shell.platform = Some(Platform::with_dialogs(LinuxDialogs::with_chooser(Arc::new(NoPortal))));
        eprintln!(
            "event=command_path_clipboard WAYLAND_DISPLAY={:?} DISPLAY={:?} backend={:?}",
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("DISPLAY"),
            shell
                .platform
                .as_ref()
                .unwrap()
                .clipboard()
                .map(LinuxClipboard::backend)
        );
        assert!(shell.power_action(Action::Copy), "Ctrl+C is handled");
        for _ in 0..2 {
            assert!(shell.power_action(Action::Paste), "Ctrl+V is handled");
            settle(shell.workspace.as_mut().unwrap(), index);
        }
        let duplicated = text(&shell, index);
        eprintln!(
            "event=command_path_clipboard document={duplicated:?} message={:?}",
            shell.workspace.as_ref().unwrap().message
        );
        for (tool, arguments) in [
            ("wl-paste", &["--no-newline"][..]),
            ("xclip", &["-o", "-selection", "clipboard"][..]),
        ] {
            let output = std::process::Command::new(tool).args(arguments).output();
            eprintln!(
                "event=command_path_clipboard reader={tool} text={:?}",
                output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            );
        }
        assert_eq!(duplicated, "alpha line\nalpha line\nbravo\n");
    }

    #[test]
    fn a_saved_path_is_replaced_only_after_the_save_dialog_confirmed_it() {
        let platform = Platform::for_tests();
        let path = Path::new("/tmp/report.txt");
        *platform.portal_confirmed.borrow_mut() = Some(path.to_path_buf());
        assert!(!platform.confirm_overwrite(Path::new("/tmp/other.txt")));
        assert!(platform.confirm_overwrite(path));
        // Once only.
        assert!(!platform.confirm_overwrite(path));
        // A destination check that never needed to ask still ends it, so a
        // later replacement of the same path is asked again.
        *platform.portal_confirmed.borrow_mut() = Some(path.to_path_buf());
        save_destination_settled(Some(&platform));
        assert!(!platform.confirm_overwrite(path));
    }
}
