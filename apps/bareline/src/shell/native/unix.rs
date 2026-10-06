// SPDX-License-Identifier: MPL-2.0
//! Linux and macOS backend of the shell seam, for now a set of stand-ins.
//!
//! The shell starts, opens its window and edits in memory. Everything the
//! adapters for these systems do not provide yet answers with the existing
//! `Unsupported` wording ("This system does not support ..."), a plain error the
//! shell already shows, or a logged no-op: nothing here panics. The real pieces
//! replace these one by one (CROSS_PLATFORM_PLAN: PR-029 renderer, PR-030 file
//! system and path trust, PR-031 and PR-032 adapters), each by editing only this
//! file.
#![allow(
    dead_code,
    reason = "stand-ins mirror the Windows adapter surface the shared shell names; some members are only reached on Windows paths"
)]
use bareline_app::task::Wake;
use bareline_platform::{
    Capability, CapabilityReport, FileIdentity, FilesystemCapability, LocalFileSystem, PlatformServices,
    SaveDialogOptions, Unsupported,
};
use bareline_renderer::{
    DrawOp, FrameStatus, LayoutError, LayoutId, Point, Rect, RenderBackend, TextBackend, TextHit, TextStyle,
};
use bareline_renderer_recording::RecordingBackend;
use std::{
    cell::Cell,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc::Sender},
};
use winit::{event_loop::EventLoopBuilder, window::Window};

#[cfg(target_os = "linux")]
use bareline_platform_linux::NativePlatform as Services;
#[cfg(target_os = "macos")]
use bareline_platform_macos::NativePlatform as Services;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("the Bareline shell supports Windows, Linux and macOS");

pub use bareline_platform::RestrictedPaths as PathTrust;
/// Session restore uses the same deny-by-default trust until PR-030.
pub use bareline_platform::RestrictedPaths as SessionPathTrust;

/// `E_NOTIMPL`, the code the shell's diagnostics record for a native operation
/// this system does not provide (the value the Windows adapter would report).
const NOT_IMPLEMENTED: i32 = -2_147_467_263;
/// The Windows `IDCANCEL` answer, which the shell treats as a cancelled dialog.
const DIALOG_CANCELLED: i32 = 2;

/// A native operation that failed, with the code the shell's diagnostics record.
#[derive(Clone, Debug)]
pub struct Error {
    code: i32,
    message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorCode(pub i32);
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    fn other(message: impl Into<String>) -> Self {
        Self {
            code: NOT_IMPLEMENTED,
            message: message.into(),
        }
    }
    pub fn code(&self) -> ErrorCode {
        ErrorCode(self.code)
    }
    pub fn message(&self) -> String {
        self.message.clone()
    }
}
impl From<Unsupported> for Error {
    fn from(unsupported: Unsupported) -> Self {
        Self::other(unsupported.to_string())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

fn unsupported(capability: Capability) -> Unsupported {
    Unsupported { capability }
}
fn unsupported_io(capability: Capability) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, unsupported(capability).to_string())
}
fn clipboard_unavailable() -> Error {
    Error::other("This system does not support the clipboard yet")
}

/// An opaque token for the editor window. The stand-ins hold no native handle;
/// it only keys per-window state the shell keeps.
pub type RawWindow = isize;
pub fn raw_window(window: &Window) -> std::result::Result<RawWindow, String> {
    Ok(u64::from(window.id()) as isize)
}

/// Native window messages do not exist here: menu commands, tray actions and
/// packet input never arrive, so the senders are dropped.
pub fn install_message_hook(
    _builder: &mut EventLoopBuilder<Wake>,
    _input_window: Rc<Cell<(isize, u64)>>,
    _commands: Sender<CommandMessage>,
    _tray: Sender<shell_integration::TrayAction>,
) {
}

/// The renderer the shell draws with. Until the portable software renderer lands
/// (PR-029: `bareline_renderer_soft::SoftRenderer`), frames go to the headless
/// `RecordingBackend`, so the window stays blank while layout, hit testing and
/// the rest of the shell run for real.
pub struct Renderer {
    /// Always software here; read by the shell's diagnostics.
    pub software: bool,
    backend: RecordingBackend,
}
/// The one place the shell's renderer is created; swapping in the software
/// renderer means changing this function and the field type above.
pub fn create_renderer(_platform: &Platform, _window: Option<&Window>, _software: bool) -> Result<Renderer> {
    Ok(Renderer::new())
}
fn layout_error(error: LayoutError) -> Error {
    Error::other(error.to_string())
}
impl Renderer {
    fn new() -> Self {
        Self {
            software: true,
            backend: RecordingBackend::default(),
        }
    }
    /// A renderer without a window, for the visual baseline cells.
    #[cfg(test)]
    pub fn offscreen(width: u32, height: u32, scale: f32) -> Result<Self> {
        let mut renderer = Self::new();
        renderer.resize(width, height, scale)?;
        Ok(renderer)
    }
    /// The recording stand-in paints nothing, so its pixels stay transparent black.
    #[cfg(test)]
    pub fn pixels_bgra(&self) -> Result<Vec<u8>> {
        let (width, height) = self.backend.size;
        Ok(vec![0; width as usize * height as usize * 4])
    }
    pub fn take_init_failure(&mut self) -> Option<(i32, bool)> {
        None
    }
    pub fn defer_hardware(&mut self) {}
    pub fn hardware_pending(&self) -> bool {
        false
    }
    pub fn refresh_fonts(&mut self) {}
    pub fn clear_brushes(&mut self) {}
    pub fn set_layout_budget(&mut self, _editors: usize, _visible_rows: usize) {}
    pub fn invalidate_device(&mut self) {
        self.backend.simulate_device_loss();
    }
}
impl RenderBackend for Renderer {
    type Error = Error;
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<()> {
        self.backend.resize(width, height, scale).map_err(layout_error)
    }
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus> {
        self.backend.render(operations).map_err(layout_error)
    }
}
impl TextBackend for Renderer {
    fn measure_text(&mut self, text: &str, size: f32) -> std::result::Result<(f32, f32), LayoutError> {
        self.backend.measure_text(text, size)
    }
    fn shape_wrapped(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape_wrapped(text, size, width, family)
    }
    fn layout_size(&self, layout: LayoutId) -> std::result::Result<(f32, f32), LayoutError> {
        self.backend.layout_size(layout)
    }
    fn shape_with_font_family(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape_with_font_family(text, size, width, family)
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape(text, size, width)
    }
    fn set_styles(&mut self, layout: LayoutId, styles: &[TextStyle]) -> std::result::Result<(), LayoutError> {
        self.backend.set_styles(layout, styles)
    }
    fn hit_test(&self, layout: LayoutId, point: Point) -> std::result::Result<TextHit, LayoutError> {
        self.backend.hit_test(layout, point)
    }
    fn caret(&self, layout: LayoutId, byte_offset: usize) -> std::result::Result<Rect, LayoutError> {
        self.backend.caret(layout, byte_offset)
    }
    fn range_rects(
        &self,
        layout: LayoutId,
        bytes: std::ops::Range<usize>,
    ) -> std::result::Result<Vec<Rect>, LayoutError> {
        self.backend.range_rects(layout, bytes)
    }
    fn release_layout(&mut self, layout: LayoutId) {
        self.backend.release_layout(layout);
    }
}

/// Installed font families for the font picker; none until a renderer that
/// draws real fonts is available.
pub struct InstalledFontFamily {
    pub name: String,
    pub monospace: bool,
}
pub fn installed_font_families() -> Vec<InstalledFontFamily> {
    Vec::new()
}

/// Answer to a "save changes?" prompt; the same shape as on Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePromptOutcome {
    Choice { choice: SaveChoice, selected: i32 },
    Failure(i32),
}
/// A native menu command; never produced here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMessage {
    pub hwnd: isize,
    pub id: usize,
    pub action: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutAction {
    License,
    ThirdPartyNotices,
    CopyDiagnostics,
}

/// Dialogs, menus and the clipboard for the editor window. Without native
/// dialogs every question takes its safe answer: nothing is discarded,
/// overwritten or run, and a save prompt reports that it is unavailable.
pub struct Platform {
    services: Services,
    clipboard_max_bytes: Cell<usize>,
}
impl Platform {
    /// # Safety
    /// None for this stand-in, which keeps no handle. The signature matches the
    /// Windows adapter (`raw` is the live editor window of this thread), so the
    /// shared call site stays the same.
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

/// Screen reader support arrives with `accesskit_unix` and `accesskit_macos`
/// (PR-033); until then the shell's accessibility tree is built but not published.
pub struct Accessibility;
impl Accessibility {
    /// # Safety
    /// None for this stand-in, which keeps no handle; see [`Platform::new`].
    pub unsafe fn new(
        _raw: RawWindow,
        _snapshot: bareline_platform::accessibility::AccessibilitySnapshot,
        _notify: Arc<dyn Fn() + Send + Sync>,
    ) -> std::result::Result<Self, &'static str> {
        eprintln!("event=accessibility_unavailable reason=no_platform_adapter");
        Ok(Self)
    }
    pub fn set_text_sources(
        &mut self,
        _sources: Vec<(u64, Arc<dyn bareline_platform::accessibility::AccessibilityTextSource>)>,
    ) {
    }
    pub fn update(&mut self, _snapshot: bareline_platform::accessibility::AccessibilitySnapshot) {}
    pub fn drain_actions(&mut self) -> Vec<bareline_platform::accessibility::AccessibilityAction> {
        Vec::new()
    }
}

/// A minimal POSIX file system until `bareline-platform-posix` lands (PR-030):
/// file identity from the device and inode numbers, plain regular-file targets
/// and rename-replace commits. Everything else keeps the trait's refusing
/// defaults, so no unverified save strategy is ever claimed.
pub struct PlaceholderFileSystem;
pub use PlaceholderFileSystem as FileSystem;
fn identity_of(metadata: &std::fs::Metadata) -> FileIdentity {
    let seconds = u64::try_from(metadata.mtime()).unwrap_or(0);
    let nanoseconds = u64::try_from(metadata.mtime_nsec()).unwrap_or(0);
    FileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
        length: metadata.len(),
        modified: seconds.saturating_mul(1_000_000_000).saturating_add(nanoseconds),
    }
}
impl PlaceholderFileSystem {
    /// Identity for external-change checks, read without opening the file.
    pub fn current_identity(&self, path: &Path) -> io::Result<FileIdentity> {
        std::fs::metadata(path).map(|metadata| identity_of(&metadata))
    }
}
impl LocalFileSystem for PlaceholderFileSystem {
    fn identity(&self, file: &std::fs::File) -> io::Result<FileIdentity> {
        file.metadata().map(|metadata| identity_of(&metadata))
    }
    fn validate_target(&self, path: &Path) -> io::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Only regular files can be saved on this system for now",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "An absolute file path is required"))?;
                if std::fs::metadata(parent)?.is_dir() {
                    Ok(())
                } else {
                    Err(io::Error::new(io::ErrorKind::NotFound, "The folder does not exist"))
                }
            }
            Err(error) => Err(error),
        }
    }
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
        if !existed && std::fs::symlink_metadata(target).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Another file appeared at this location before the save finished",
            ));
        }
        std::fs::rename(staged, target)?;
        // Make the rename durable: the directory entry lives in the parent.
        match target.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            Some(parent) => std::fs::File::open(parent)?.sync_all(),
            None => Ok(()),
        }
    }
}
impl FilesystemCapability for PlaceholderFileSystem {
    fn report(&self, _path: &Path) -> io::Result<CapabilityReport> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "File system capabilities are not reported on this system yet",
        ))
    }
}

/// File watching arrives with `notify` (inotify, FSEvents) in PR-031 and PR-032.
pub struct WatchService;
impl WatchService {
    pub fn start_notifying(_paths: Vec<PathBuf>, _notify: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        Err(unsupported_io(Capability::FileWatch))
    }
    pub fn try_recv(&self) -> Option<bareline_platform::WatchEvent> {
        None
    }
}

/// Runs no program: process containment for these systems is not built yet.
pub struct ProcessLauncher;
impl bareline_app::macros::model::process::ProcessLauncher for ProcessLauncher {
    fn spawn(
        &self,
        _command: &mut std::process::Command,
    ) -> io::Result<(
        std::process::Child,
        Box<dyn bareline_app::macros::model::process::ProcessTreeGuard>,
    )> {
        Err(unsupported_io(Capability::Shell))
    }
}
pub fn resolve_program(_name: &str) -> std::result::Result<PathBuf, String> {
    Err(unsupported(Capability::Shell).to_string())
}

/// Never deletes: without a trash implementation the entry stays where it is.
pub fn recycle_entry(_fs: &dyn LocalFileSystem, _path: &Path, _owner: RawWindow) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This system does not support moving files to the trash yet",
    ))
}

/// Logoff and shutdown reach Bareline as SIGTERM or SIGHUP on these systems
/// (ADR-C); until that handler exists the signal state is kept but never fed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndMessage {
    /// The session may end. Answered at once and never vetoed.
    Query,
    /// The session ends; `ending` is false when it was cancelled.
    End { ending: bool },
}
/// Side effects of a session-end notification.
pub trait SessionEndHost {
    /// Route one flush request to the application.
    fn deliver(&mut self);
    /// Ask the system for more time. Returns whether it took effect.
    fn block(&mut self) -> bool;
    fn unblock(&mut self);
}
/// UI-thread state shared by the session-end source and the application; the
/// same protocol as the Windows adapter's.
#[derive(Debug, Default)]
pub struct SessionEndSignal {
    requests: Cell<u32>,
    flushed: Cell<bool>,
    dirty: Cell<bool>,
    blocked: Cell<bool>,
}
impl SessionEndSignal {
    pub fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
    }
    /// Claim one routed flush request. False means an ordinary close request.
    pub fn take_request(&self) -> bool {
        let requests = self.requests.get();
        if requests == 0 {
            return false;
        }
        self.requests.set(requests - 1);
        true
    }
    pub fn finish(&self, complete: bool) {
        self.flushed.set(complete);
    }
    /// Answer one notification and return its result.
    pub fn respond(&self, host: &mut dyn SessionEndHost, message: SessionEndMessage) -> isize {
        match message {
            SessionEndMessage::Query => {
                if self.dirty.get() && !self.blocked.get() {
                    self.blocked.set(host.block());
                }
                self.request(host);
                1
            }
            SessionEndMessage::End { ending } => {
                if ending && !self.flushed.get() {
                    self.request(host);
                }
                if self.blocked.replace(false) {
                    host.unblock();
                }
                0
            }
        }
    }
    fn request(&self, host: &mut dyn SessionEndHost) {
        self.flushed.set(false);
        self.requests.set(self.requests.get().saturating_add(1));
        host.deliver();
    }
}
pub struct SessionEndMonitor;
impl SessionEndMonitor {
    /// # Safety
    /// None for this stand-in, which keeps no handle; see [`Platform::new`].
    pub unsafe fn attach(_raw: RawWindow, _signal: Rc<SessionEndSignal>) -> Result<Self> {
        Err(Error::other(
            "This system does not report logoff or shutdown to Bareline yet",
        ))
    }
}
/// Restart registration exists for Windows update restarts; these systems
/// update through their package managers, so there is nothing to register.
pub fn register_application_restart() -> Result<()> {
    Ok(())
}

pub fn high_contrast_enabled() -> io::Result<bool> {
    Ok(false)
}
pub fn high_contrast_highlight() -> Option<(u32, u32)> {
    None
}
/// The user's language from the POSIX locale variables as a BCP 47 name such as
/// `de-DE`. `None` for the C/POSIX locale or when nothing is set.
pub fn system_ui_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .and_then(|value| locale_name(value.to_str()?))
}
/// `de_DE.UTF-8` or `sr_RS@latin` to `de-DE` or `sr-RS`.
fn locale_name(value: &str) -> Option<String> {
    let name = value.split(['.', '@']).next().unwrap_or_default().replace('_', "-");
    (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}
/// The code page legacy text falls back to: UTF-8 on these systems.
pub fn system_code_page() -> u32 {
    65001
}
pub fn spell_checker_factory() -> bareline_platform::spelling::SpellCheckerFactory {
    Arc::new(|| Err("This system does not support spell checking yet".to_owned()))
}
/// No process-independent clock is exposed yet; performance runs fail closed.
pub fn monotonic_ns() -> Option<u128> {
    None
}
pub fn private_bytes() -> Result<u64> {
    Err(Error::other(
        "Process memory counters are not available on this system yet",
    ))
}

/// The longest path the handoff accepts, in bytes (Linux `PATH_MAX`).
const MAX_PATH_BYTES: usize = 4096;
/// The same limits the instance handoff enforces for every forwarded path.
pub fn valid_launch_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    path.is_absolute() && !bytes.is_empty() && !bytes.contains(&0) && bytes.len() <= MAX_PATH_BYTES
}
/// GDI and USER objects do not exist here and descriptors are not counted yet.
pub fn handle_counters() -> (u32, u32, u32) {
    (0, 0, 0)
}

pub mod cli {
    //! Command-line output goes to the standard streams of the invoking shell.
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
}

pub mod instance {
    //! Single-instance handoff (a Unix socket lock, ADR-C) is not built yet:
    //! every window runs on its own and leaves the shared session alone.
    use std::{
        io,
        path::{Path, PathBuf},
        sync::Arc,
    };

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct OpenRequest {
        pub paths: Vec<PathBuf>,
        pub line: Option<u64>,
        pub column: Option<u64>,
        pub read_only: bool,
        pub monitor: bool,
    }
    pub enum Outcome {
        Forwarded,
        Primary(InstanceServer),
        Independent(String),
    }
    pub struct InstanceServer;
    impl InstanceServer {
        pub fn try_recv(&self) -> Option<OpenRequest> {
            None
        }
        pub fn set_accepting(&self, _accepting: bool) {}
        pub fn pending(&self) -> usize {
            0
        }
        pub fn quiesce(&self) -> usize {
            0
        }
    }
    pub fn coordinate(
        _scope: &Path,
        _profile: Option<&Path>,
        _request: OpenRequest,
        _new_instance: bool,
        _notify: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Outcome> {
        Ok(Outcome::Independent(
            "This system does not support handing files to a running Bareline window yet, so this window runs separately; its tabs are not restored next time.".into(),
        ))
    }
}

pub mod extension_transport {
    //! The extension host transport (a Unix domain socket with peer credentials,
    //! ADR-C) is not built yet; no extension host is started.
    use bareline_extensions_protocol::{BrokerResponse, Envelope, ExecutionBudget, Invocation};
    use std::{
        io,
        path::Path,
        sync::{Arc, atomic::AtomicBool},
    };

    pub struct HostLaunch<'a> {
        pub executable: &'a Path,
        pub executable_sha256: [u8; 32],
        pub signer: &'a bareline_distribution::update::PublisherPin,
        pub component: &'a Path,
        pub component_sha256: [u8; 32],
        pub invocation: &'a Invocation,
        pub budget: ExecutionBudget,
    }
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
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "This system does not support running extensions yet",
        ))
    }
}

pub mod printing {
    //! Printing is a later tier on these systems (ADR-C tier 3).
    use bareline_platform::{
        Capability, Unsupported,
        printing::{PrintError, PrintLine, PrintOptions, PrintSummary, PrintTarget},
    };
    use std::sync::atomic::AtomicBool;

    fn unavailable() -> PrintError {
        PrintError::Unavailable(
            Unsupported {
                capability: Capability::Printing,
            }
            .to_string(),
        )
    }
    pub struct PrinterSelection;
    pub fn choose_printer(_owner: Option<super::RawWindow>) -> Result<Option<PrinterSelection>, PrintError> {
        Err(unavailable())
    }
    pub struct PrintJob;
    impl PrintJob {
        pub fn start(_selection: PrinterSelection, _options: PrintOptions) -> Result<Self, PrintError> {
            Err(unavailable())
        }
    }
    impl PrintTarget for PrintJob {
        fn write_line(&mut self, _line: PrintLine<'_>, _cancel: &AtomicBool) -> Result<(), PrintError> {
            Err(unavailable())
        }
        fn finish(self: Box<Self>, _cancel: &AtomicBool) -> Result<PrintSummary, PrintError> {
            Err(unavailable())
        }
    }
}

pub mod shell_integration {
    //! Reveal, terminal, recent items, the jump list and the tray icon.
    use bareline_platform::{Capability, Unsupported};
    use std::path::Path;

    fn unavailable(capability: Capability) -> String {
        Unsupported { capability }.to_string()
    }
    pub fn reveal(_path: &Path) -> Result<(), String> {
        Err(unavailable(Capability::Shell))
    }
    /// Only Windows searches the launch directory for libraries.
    pub fn harden_process_search_paths() -> Result<(), String> {
        Ok(())
    }
    pub fn is_network_path(_path: &Path) -> bool {
        false
    }
    pub fn open_terminal(_directory: &Path) -> Result<(), String> {
        Err(unavailable(Capability::Shell))
    }
    /// Desktop recent-items integration is not built yet; Bareline's own Recent
    /// Files list is unaffected.
    pub fn add_recent(_path: &Path, _portable: bool, _enabled: bool) {}
    /// Jump lists are a Windows taskbar feature.
    pub fn initialize_jump_list(_portable: bool) -> Result<(), String> {
        Ok(())
    }
    #[derive(Clone, Copy, Debug)]
    pub enum TrayAction {
        Restore,
        New,
        Open,
        Find,
        Exit,
    }
    pub struct TrayIcon;
    impl TrayIcon {
        pub fn new(_window: super::RawWindow) -> Result<Self, String> {
            Err(unavailable(Capability::Tray))
        }
    }
}

pub mod update {
    //! In-app updates are a Windows feature; these systems update through their
    //! package managers (ADR-C tier 3). Every entry point reports `Unsupported`,
    //! starting with the installation check that each update flow runs first.
    use bareline_distribution::update::{Manifest, PublisherPin, TrustPolicy, VerifiedManifest};
    use bareline_platform::{Capability, Unsupported};
    use std::{
        fs::File,
        io,
        path::{Path, PathBuf},
        sync::atomic::AtomicBool,
    };

    fn unsupported() -> io::Error {
        super::unsupported_io(Capability::Update)
    }
    pub const UPDATE_LAUNCH_ATTEMPTS: u32 = 3;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AuthorityFreshness {
        Required,
        Installed,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum LaunchDecision {
        NotPending,
        Counted(u32),
        Recover,
        RecoveryFailed,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum HelperAction {
        Apply,
        Acknowledge,
        Recover,
        AutoRecover,
    }
    #[derive(Clone, Copy, Debug)]
    pub struct OfflineRootPolicy<'a> {
        pub public_key: &'a str,
        pub minimum_version: u64,
    }
    pub struct ResolvedReleaseAuthority {
        pub release_public_key: String,
        pub signer: PublisherPin,
        pub update_helper_sha256: Option<String>,
        pub minimum_metadata_version: u64,
        pub minimum_runtime_metadata_version: u64,
        pub minimum_catalog_metadata_version: u64,
        pub catalog_public_key: Option<String>,
    }
    pub struct PreparedUpdate {
        pub manifest: VerifiedManifest,
        pub file: File,
        pub directory: PathBuf,
    }
    #[derive(Clone, Debug)]
    pub struct InstalledRuntime {
        pub executable: PathBuf,
        pub executable_sha256: [u8; 32],
        pub signer: PublisherPin,
        pub version: String,
        pub metadata_version: u64,
    }
    pub fn validate_install_root(_root: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn update_state_root(_root: &Path) -> io::Result<PathBuf> {
        Err(unsupported())
    }
    #[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
    pub fn resolve_release_authority(
        _root: &Path,
        _state: &Path,
        _embedded_key: &str,
        _embedded_signer: &PublisherPin,
        _embedded_floor: u64,
        _offline_policy: Option<OfflineRootPolicy<'_>>,
        _freshness: AuthorityFreshness,
        _now: u64,
    ) -> io::Result<ResolvedReleaseAuthority> {
        Err(unsupported())
    }
    pub fn record_update_launch(_root: &Path, _state: &Path, _limit: u32) -> io::Result<LaunchDecision> {
        Err(unsupported())
    }
    pub fn launch_update_helper(
        _root: &Path,
        _authority: &ResolvedReleaseAuthority,
        _action: HelperAction,
    ) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn core_metadata_floor(_root: &Path, _state: &Path, _authority_floor: u64) -> io::Result<u64> {
        Err(unsupported())
    }
    #[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
    pub fn fetch_verified_update(
        _host: &str,
        _manifest_path: &str,
        _signature_path: &str,
        _artifact_path: &str,
        _policy: &TrustPolicy<'_>,
        _now_unix: u64,
        _signer: &PublisherPin,
        _stage_parent: &Path,
        _cancel: &AtomicBool,
    ) -> Result<PreparedUpdate, Unsupported> {
        Err(Unsupported {
            capability: Capability::Update,
        })
    }
    pub fn verify_delivered_trust(
        _directory: &Path,
        _root: &Path,
        _state: &Path,
        _manifest: &Manifest,
        _policy: OfflineRootPolicy<'_>,
        _core: &File,
        _now: u64,
    ) -> io::Result<Option<()>> {
        Err(unsupported())
    }
    pub fn discard_prepared_update(_prepared: PreparedUpdate) {}
    pub fn transfer_update(_prepared: PreparedUpdate, _root: &Path, _state: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn recovery_source(_root: &Path, _state: &Path) -> io::Result<Option<()>> {
        Err(unsupported())
    }
    pub fn discard_pending_update(_root: &Path, _state: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn restore_verified_runtime(
        _extensions_root: &Path,
        _hash: &str,
        _trust: &TrustPolicy<'_>,
        _now: u64,
        _publisher: &PublisherPin,
    ) -> io::Result<InstalledRuntime> {
        Err(unsupported())
    }
    #[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
    pub fn install_verified_runtime(
        _executable_path: &Path,
        _metadata_bytes: &[u8],
        _signature_text: &str,
        _trust: &TrustPolicy<'_>,
        _now: u64,
        _publisher: &PublisherPin,
        _extensions_root: &Path,
        _cancel: &AtomicBool,
    ) -> io::Result<InstalledRuntime> {
        Err(unsupported())
    }
    pub fn remove_verified_runtime(_runtime: &InstalledRuntime) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bareline-native-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn platform() -> Platform {
        Platform {
            services: Services,
            clipboard_max_bytes: Cell::new(64),
        }
    }

    #[test]
    fn commit_replaces_by_rename_and_identity_follows_the_new_file() {
        let root = scratch("commit");
        let (target, staged) = (root.join("note.txt"), root.join("note.txt.stage"));
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&staged, "new").unwrap();
        let before = FileSystem.current_identity(&target).unwrap();
        assert_eq!(
            FileSystem.identity(&std::fs::File::open(&target).unwrap()).unwrap(),
            before
        );
        FileSystem.validate_target(&target).unwrap();
        FileSystem.commit(&staged, &target, true).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!staged.exists());
        // The name now holds the staged file, which coexisted with the old one.
        let after = FileSystem.current_identity(&target).unwrap();
        assert_eq!(after.volume, before.volume);
        assert_ne!(after.file, before.file);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_never_replaces_a_file_that_appeared_and_targets_are_regular_files() {
        let root = scratch("refuse");
        let (target, staged) = (root.join("late.txt"), root.join("late.txt.stage"));
        std::fs::write(&staged, "mine").unwrap();
        // A new file in an existing folder is a valid target.
        FileSystem.validate_target(&target).unwrap();
        std::fs::write(&target, "theirs").unwrap();
        let error = FileSystem.commit(&staged, &target, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "theirs");
        assert!(staged.exists());
        assert!(FileSystem.validate_target(&root).is_err());
        assert!(
            FileSystem
                .validate_target(&root.join("missing").join("new.txt"))
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locale_names_become_language_tags() {
        assert_eq!(locale_name("de_DE.UTF-8").as_deref(), Some("de-DE"));
        assert_eq!(locale_name("sr_RS@latin").as_deref(), Some("sr-RS"));
        assert_eq!(locale_name("en").as_deref(), Some("en"));
        assert_eq!(locale_name("C.UTF-8"), None);
        assert_eq!(locale_name("POSIX"), None);
        assert_eq!(locale_name(""), None);
    }

    #[test]
    fn launch_paths_follow_posix_limits() {
        assert!(valid_launch_path(Path::new("/home/user/notes.txt")));
        assert!(!valid_launch_path(Path::new("notes.txt")));
        assert!(!valid_launch_path(&Path::new("/").join("a".repeat(MAX_PATH_BYTES))));
    }

    #[test]
    fn session_end_routes_one_flush_per_notification() {
        #[derive(Default)]
        struct Host {
            delivered: u32,
            blocked: bool,
        }
        impl SessionEndHost for Host {
            fn deliver(&mut self) {
                self.delivered += 1;
            }
            fn block(&mut self) -> bool {
                self.blocked = true;
                true
            }
            fn unblock(&mut self) {
                self.blocked = false;
            }
        }
        let signal = SessionEndSignal::default();
        let mut host = Host::default();
        signal.set_dirty(true);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::Query), 1);
        assert!(host.blocked);
        assert!(signal.take_request());
        assert!(!signal.take_request());
        // An unfinished flush is requested again when the session really ends.
        signal.finish(false);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::End { ending: true }), 0);
        assert_eq!(host.delivered, 2);
        assert!(!host.blocked);
    }

    #[test]
    fn missing_native_features_answer_in_plain_language() {
        let platform = platform();
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
        assert!(resolve_program("sh").unwrap_err().starts_with(refusal));
        // Questions take their safe answer: nothing is overwritten, discarded or closed.
        assert!(!platform.confirm_overwrite(Path::new("/tmp/kept.txt")));
        assert!(!platform.confirm_discard_and_reload("kept.txt"));
        assert_eq!(
            platform.confirm_save_document("kept.txt"),
            SavePromptOutcome::Failure(NOT_IMPLEMENTED)
        );
        assert_eq!(
            update::validate_install_root(Path::new("/opt/bareline"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert!(WatchService::start_notifying(vec![PathBuf::from("/tmp")], Arc::new(|| {})).is_err());
    }

    #[test]
    fn stand_in_renderer_lays_out_text_and_reports_device_loss() {
        let mut renderer = create_renderer(&platform(), None, false).unwrap();
        assert!(renderer.software && !renderer.hardware_pending());
        renderer.resize(320, 200, 1.0).unwrap();
        let layout = renderer.shape("Bareline", 14.0, 300.0).unwrap();
        assert!(renderer.layout_size(layout).unwrap().0 > 0.0);
        assert_eq!(renderer.render(&[]).unwrap(), FrameStatus::Presented);
        renderer.invalidate_device();
        assert_eq!(renderer.render(&[]).unwrap(), FrameStatus::Recreate);
        renderer.release_layout(layout);
    }
}
