// SPDX-License-Identifier: MPL-2.0
//! Open, Save and folder dialogs through the XDG desktop portal's FileChooser,
//! which every current desktop provides (GNOME, KDE, wlroots and others) and
//! which needs no GTK in this process. Without a portal the dialogs report the
//! capability's plain-language `Unsupported` refusal (UI-03).
//!
//! Blocking model: a portal dialog is a window of another process. Asking for
//! one is a D-Bus call that returns at once, and the answer arrives as a signal
//! when the user closes the dialog, which can take minutes. [`LinuxDialogs::begin`]
//! therefore runs the whole exchange on a short-lived worker thread and returns
//! a [`PendingDialog`] the event loop polls; its `notify` callback wakes the loop
//! when the answer is in. The event loop keeps dispatching meanwhile, so the
//! window keeps repainting and answering the compositor's pings (a window that
//! stops answering is flagged "not responding" after a few seconds on GNOME).
//! The synchronous [`PlatformServices`] methods wait on the same worker; they
//! never stall the compositor or the portal, which run in other processes, but
//! they do pause the calling thread until the user closes the dialog, so the
//! event loop must use `begin` and poll the [`PendingDialog`], never them.
//!
//! Overwrite: the portal always asks before a Save replaces an existing file and
//! has no option to leave that to the application, so with `app_confirms_overwrite`
//! the user may be asked twice (by the portal for the typed name, then by the
//! shell for the final path). The default extension is added only when that
//! cannot skip a confirmation; see `with_default_extension`.
use crate::portal::{ChooserMethod, ChooserOptions, DesktopPortal, FileChooser, Filter, PortalError, path_from_uri};
use bareline_platform::{
    Capability, FileTypeFilter, PlatformServices, SaveDialogOptions, Unsupported,
    dialogs::{ALL_FILES, TEXT_FILES},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// What a dialog chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogRequest {
    /// One file, or several with `multiple`.
    Open {
        multiple: bool,
    },
    Save(SaveDialogOptions),
    PickFolder,
}
impl DialogRequest {
    fn capability(&self) -> Capability {
        match self {
            Self::Open { .. } => Capability::OpenFile,
            Self::Save(_) => Capability::SaveFile,
            Self::PickFolder => Capability::PickFolder,
        }
    }
}
/// The chosen paths (empty when the user cancelled), or a readable failure.
pub type DialogResult = Result<Vec<PathBuf>, String>;

/// A dialog in progress on its worker thread.
pub struct PendingDialog {
    receiver: Receiver<DialogResult>,
    done: Option<DialogResult>,
}
impl PendingDialog {
    /// The answer once the dialog closed; `None` while it is still open.
    pub fn try_result(&mut self) -> Option<DialogResult> {
        if self.done.is_none() {
            match self.receiver.try_recv() {
                Ok(result) => self.done = Some(result),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => self.done = Some(Err("The dialog stopped unexpectedly.".into())),
            }
        }
        self.done.take()
    }
    /// Waits for the answer on this thread.
    pub fn wait(mut self) -> DialogResult {
        if let Some(result) = self.done.take() {
            return result;
        }
        self.receiver
            .recv()
            .unwrap_or_else(|_| Err("The dialog stopped unexpectedly.".into()))
    }
}

type ChooserSource = Arc<dyn Fn() -> Result<Arc<dyn FileChooser>, PortalError> + Send + Sync>;

/// The portal dialogs of this session.
#[derive(Clone)]
pub struct LinuxDialogs {
    chooser: ChooserSource,
    /// The FileChooser `parent_window`: `x11:<xid>`, or empty.
    parent: String,
}
impl Default for LinuxDialogs {
    fn default() -> Self {
        Self::new()
    }
}
impl LinuxDialogs {
    /// Dialogs through the session's portal. The bus connection is made on the
    /// first dialog's worker, never on the caller's thread, and then reused.
    /// Only a connection is kept: when the session bus was not reachable, the
    /// next dialog tries again rather than reporting `Unsupported` for good.
    pub fn new() -> Self {
        let portal: Arc<Mutex<Option<DesktopPortal>>> = Arc::default();
        Self::with_chooser_source(Arc::new(move || {
            let mut cached = portal.lock().unwrap_or_else(PoisonError::into_inner);
            let portal = match cached.as_ref() {
                Some(portal) => portal.clone(),
                None => cached.insert(DesktopPortal::session()?).clone(),
            };
            Ok(portal.file_chooser())
        }))
    }
    /// Dialogs through a given FileChooser (another bus, or a test double).
    pub fn with_chooser(chooser: Arc<dyn FileChooser>) -> Self {
        Self::with_chooser_source(Arc::new(move || Ok(chooser.clone())))
    }
    fn with_chooser_source(chooser: ChooserSource) -> Self {
        Self {
            chooser,
            parent: String::new(),
        }
    }
    /// Attaches the dialogs to the editor window (see `portal::x11_parent`).
    pub fn set_parent(&mut self, parent: String) {
        self.parent = parent;
    }
    /// Starts a dialog on its own worker and returns at once; `notify` runs on
    /// the worker when the answer is ready (typically an event loop wake-up).
    pub fn begin(&self, request: DialogRequest, notify: Arc<dyn Fn() + Send + Sync>) -> PendingDialog {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (source, parent) = (self.chooser.clone(), self.parent.clone());
        let spawned = std::thread::Builder::new()
            .name("bareline-dialog".into())
            .spawn(move || {
                let _ = sender.send(run(&source, &parent, &request));
                notify();
            });
        let done = spawned
            .err()
            .map(|error| Err(format!("The dialog could not start ({error}).")));
        PendingDialog { receiver, done }
    }
    fn blocking(&self, request: DialogRequest) -> DialogResult {
        self.begin(request, Arc::new(|| {})).wait()
    }
    /// Several files from one Open dialog; empty when cancelled.
    pub fn open_files(&self) -> Result<Vec<PathBuf>, String> {
        self.blocking(DialogRequest::Open { multiple: true })
    }
}
impl PlatformServices for LinuxDialogs {
    /// The About window is drawn by the shell; see `prompts::InAppPrompt::about`.
    fn about(&self) {
        eprintln!(
            "{}",
            Unsupported {
                capability: Capability::About
            }
        );
    }
    fn open_file(&self) -> Result<Option<PathBuf>, String> {
        self.blocking(DialogRequest::Open { multiple: false })
            .map(|paths| paths.into_iter().next())
    }
    fn save_file(&self) -> Result<Option<PathBuf>, String> {
        self.save_file_with(&SaveDialogOptions::new(bareline_platform::SaveFileKind::Any))
    }
    fn save_file_with(&self, options: &SaveDialogOptions) -> Result<Option<PathBuf>, String> {
        self.blocking(DialogRequest::Save(options.clone()))
            .map(|paths| paths.into_iter().next())
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String> {
        self.blocking(DialogRequest::PickFolder)
            .map(|paths| paths.into_iter().next())
    }
}

/// `*.txt;*.md` as portal globs. Windows' `*.*` means every file, but as a glob
/// it would demand a dot, so `Makefile` would not show; it becomes `*`.
fn filter(filter: &FileTypeFilter) -> Filter {
    let patterns = filter
        .patterns
        .split(';')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            (
                0,
                if pattern == "*.*" {
                    "*".to_owned()
                } else {
                    pattern.to_owned()
                },
            )
        })
        .collect();
    (filter.label.to_owned(), patterns)
}
fn options_for(request: &DialogRequest) -> (ChooserMethod, &'static str, ChooserOptions) {
    match request {
        DialogRequest::Open { multiple } => {
            let filters: Vec<Filter> = [TEXT_FILES, ALL_FILES].iter().map(filter).collect();
            (
                ChooserMethod::OpenFile,
                "Open",
                ChooserOptions {
                    multiple: *multiple,
                    current_filter: filters.first().cloned(),
                    filters,
                    ..Default::default()
                },
            )
        }
        DialogRequest::Save(options) => {
            let filters: Vec<Filter> = options.filters().iter().map(filter).collect();
            (
                ChooserMethod::SaveFile,
                "Save As",
                ChooserOptions {
                    current_filter: filters.first().cloned(),
                    filters,
                    current_name: options.default_name.clone(),
                    // The caller checked it on a worker (APP-19); passed as given.
                    current_folder: options.default_directory.as_deref().map(crate::portal::folder_bytes),
                    ..Default::default()
                },
            )
        }
        DialogRequest::PickFolder => (
            ChooserMethod::OpenFile,
            "Select Folder",
            ChooserOptions {
                directory: true,
                accept_label: Some("Select Folder".into()),
                ..Default::default()
            },
        ),
    }
}
/// A typed name without an extension gets the kind's default one, as the
/// Windows dialog's default extension does; `Makefile` saved as a named
/// document keeps its name because that kind has none.
///
/// The portal confirmed an overwrite (if any) for the name the user typed, not
/// for the extended one, unlike Windows, where the extension is added before
/// the dialog's own overwrite prompt. So the extension is added only when that
/// cannot replace a file nobody confirmed: when the shell confirms the final
/// path itself (`app_confirms_overwrite`), or when nothing exists under the
/// extended name. Otherwise the path is returned exactly as chosen. This runs on
/// the dialog worker, so the `lstat` never touches the event loop.
fn with_default_extension(path: PathBuf, options: &SaveDialogOptions) -> PathBuf {
    match options.default_extension() {
        Some(extension) if path.extension().is_none() && path.file_name().is_some() => {
            let mut name = path.file_name().unwrap_or_default().to_owned();
            name.push(".");
            name.push(extension);
            let extended = path.with_file_name(name);
            let unclaimed = matches!(
                std::fs::symlink_metadata(&extended),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            );
            if options.app_confirms_overwrite || unclaimed {
                extended
            } else {
                path
            }
        }
        _ => path,
    }
}
fn run(source: &ChooserSource, parent: &str, request: &DialogRequest) -> DialogResult {
    let unsupported = || {
        Unsupported {
            capability: request.capability(),
        }
        .to_string()
    };
    let chooser = match source() {
        Ok(chooser) => chooser,
        Err(PortalError::Unavailable(_)) => return Err(unsupported()),
        Err(error) => return Err(error.to_string()),
    };
    let (method, title, options) = options_for(request);
    let response = match chooser.choose(method, parent, title, &options) {
        Ok(response) => response,
        Err(PortalError::Unavailable(_)) => return Err(unsupported()),
        Err(error) => return Err(error.to_string()),
    };
    match response.code {
        0 => {}
        1 => return Ok(Vec::new()),
        _ => return Err("The dialog closed without a choice.".into()),
    }
    let mut paths = Vec::with_capacity(response.uris.len());
    for uri in &response.uris {
        let path = path_from_uri(uri).ok_or_else(|| "The chosen location is not a local file.".to_string())?;
        paths.push(match request {
            DialogRequest::Save(options) => with_default_extension(path, options),
            _ => path,
        });
    }
    if paths.is_empty() {
        return Err("The dialog returned no location.".into());
    }
    Ok(paths)
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::SaveFileKind;

    /// Answers every request with a fixed response and records what was asked.
    struct Fake {
        answer: Result<crate::portal::ChooserResponse, PortalError>,
        asked: Mutex<Vec<(ChooserMethod, String, String, ChooserOptions)>>,
    }
    impl Fake {
        fn answering(code: u32, uris: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                answer: Ok(crate::portal::ChooserResponse {
                    code,
                    uris: uris.iter().map(|uri| uri.to_string()).collect(),
                }),
                asked: Mutex::default(),
            })
        }
        fn last(&self) -> (ChooserMethod, String, String, ChooserOptions) {
            self.asked.lock().unwrap().last().cloned().unwrap()
        }
    }
    impl FileChooser for Fake {
        fn choose(
            &self,
            method: ChooserMethod,
            parent: &str,
            title: &str,
            options: &ChooserOptions,
        ) -> Result<crate::portal::ChooserResponse, PortalError> {
            self.asked
                .lock()
                .unwrap()
                .push((method, parent.into(), title.into(), options.clone()));
            self.answer.clone()
        }
    }

    #[test]
    fn open_dialog_returns_the_chosen_files_and_nothing_on_cancel() {
        let fake = Fake::answering(0, &["file:///home/u/a.txt", "file:///home/u/b%20c.md"]);
        let mut dialogs = LinuxDialogs::with_chooser(fake.clone());
        dialogs.set_parent("x11:2a".into());
        assert_eq!(
            dialogs.open_files().unwrap(),
            vec![PathBuf::from("/home/u/a.txt"), PathBuf::from("/home/u/b c.md")]
        );
        let (method, parent, _, options) = fake.last();
        assert_eq!((method, parent.as_str()), (ChooserMethod::OpenFile, "x11:2a"));
        assert!(options.multiple && !options.directory);
        // Text files first, then every file, as `*` rather than `*.*`.
        assert_eq!(options.filters[0].0, "Text files");
        assert_eq!(options.filters[1], ("All files".to_owned(), vec![(0, "*".to_owned())]));
        assert_eq!(options.current_filter.as_ref(), options.filters.first());
        assert_eq!(dialogs.open_file().unwrap(), Some(PathBuf::from("/home/u/a.txt")));
        assert!(!fake.last().3.multiple);
        let cancelled = LinuxDialogs::with_chooser(Fake::answering(1, &[]));
        assert_eq!(cancelled.open_file().unwrap(), None);
        assert_eq!(cancelled.pick_folder().unwrap(), None);
    }
    #[test]
    fn save_dialog_carries_its_options_and_adds_the_default_extension() {
        let fake = Fake::answering(0, &["file:///docs/notes"]);
        let dialogs = LinuxDialogs::with_chooser(fake.clone());
        let options = SaveDialogOptions::new(SaveFileKind::Text)
            .named("Untitled 1.txt")
            .in_directory(Some(PathBuf::from("/docs")));
        assert_eq!(
            dialogs.save_file_with(&options).unwrap(),
            Some(PathBuf::from("/docs/notes.txt"))
        );
        let (method, _, title, sent) = fake.last();
        assert_eq!((method, title.as_str()), (ChooserMethod::SaveFile, "Save As"));
        assert_eq!(sent.current_name.as_deref(), Some("Untitled 1.txt"));
        assert_eq!(sent.current_folder.as_deref(), Some(&b"/docs"[..]));
        assert_eq!(sent.filters[0].1[0], (0, "*.txt".to_owned()));
        // A named document keeps a name typed without an extension.
        let named = SaveDialogOptions::new(SaveFileKind::Named).named("Makefile");
        let fake = Fake::answering(0, &["file:///src/Makefile"]);
        let dialogs = LinuxDialogs::with_chooser(fake);
        assert_eq!(
            dialogs.save_file_with(&named).unwrap(),
            Some(PathBuf::from("/src/Makefile"))
        );
        // A typed extension is never doubled.
        let fake = Fake::answering(0, &["file:///docs/page.htm"]);
        let html = SaveDialogOptions::new(SaveFileKind::Html);
        assert_eq!(
            LinuxDialogs::with_chooser(fake).save_file_with(&html).unwrap(),
            Some(PathBuf::from("/docs/page.htm"))
        );
    }
    #[test]
    fn the_default_extension_never_replaces_a_file_nobody_confirmed() {
        let folder = std::env::temp_dir().join(format!("bareline-dialogs-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let existing = folder.join("session.json");
        std::fs::write(&existing, b"{}").unwrap();
        let chosen = folder.join("session");
        let uri = format!("file://{}", chosen.display());
        let json = SaveDialogOptions::new(SaveFileKind::Json);
        assert_eq!(json.default_extension(), Some("json"));
        // The portal confirmed `session`; `session.json` exists, so it is not used.
        let dialogs = LinuxDialogs::with_chooser(Fake::answering(0, &[uri.as_str()]));
        assert_eq!(dialogs.save_file_with(&json).unwrap(), Some(chosen.clone()));
        // When the shell confirms the final path itself, the extension is added.
        let confirmed = json.clone().app_confirms_overwrite();
        assert_eq!(dialogs.save_file_with(&confirmed).unwrap(), Some(existing.clone()));
        // With nothing under the extended name, it is added as before.
        std::fs::remove_file(&existing).unwrap();
        assert_eq!(dialogs.save_file_with(&json).unwrap(), Some(existing));
        std::fs::remove_dir_all(&folder).unwrap();
    }
    #[test]
    fn folder_picker_asks_for_a_directory() {
        let fake = Fake::answering(0, &["file:///home/u/project"]);
        let dialogs = LinuxDialogs::with_chooser(fake.clone());
        assert_eq!(dialogs.pick_folder().unwrap(), Some(PathBuf::from("/home/u/project")));
        let (method, _, _, options) = fake.last();
        assert_eq!(method, ChooserMethod::OpenFile);
        assert!(options.directory && !options.multiple);
        assert!(options.filters.is_empty());
    }
    #[test]
    fn a_missing_portal_is_unsupported_and_other_failures_say_why() {
        let missing = Arc::new(Fake {
            answer: Err(PortalError::Unavailable("ServiceUnknown".into())),
            asked: Mutex::default(),
        });
        let dialogs = LinuxDialogs::with_chooser(missing);
        for (result, capability) in [
            (dialogs.open_file(), Capability::OpenFile),
            (dialogs.save_file(), Capability::SaveFile),
            (dialogs.pick_folder(), Capability::PickFolder),
        ] {
            assert_eq!(result.unwrap_err(), Unsupported { capability }.to_string());
        }
        let no_bus = LinuxDialogs::with_chooser_source(Arc::new(|| Err(PortalError::Unavailable("no bus".into()))));
        assert!(
            no_bus
                .open_file()
                .unwrap_err()
                .starts_with("This system does not support ")
        );
        let remote = LinuxDialogs::with_chooser(Fake::answering(0, &["sftp://host/file"]));
        assert_eq!(
            remote.open_file().unwrap_err(),
            "The chosen location is not a local file."
        );
        let ended = LinuxDialogs::with_chooser(Fake::answering(2, &[]));
        assert!(ended.open_file().is_err());
    }
    #[test]
    fn begin_returns_at_once_and_notifies_when_the_dialog_closes() {
        /// A dialog the test closes by hand.
        struct Held(Mutex<Receiver<()>>);
        impl FileChooser for Held {
            fn choose(
                &self,
                _: ChooserMethod,
                _: &str,
                _: &str,
                _: &ChooserOptions,
            ) -> Result<crate::portal::ChooserResponse, PortalError> {
                self.0.lock().unwrap().recv().unwrap();
                Ok(crate::portal::ChooserResponse {
                    code: 0,
                    uris: vec!["file:///late.txt".into()],
                })
            }
        }
        let (close, closed) = mpsc::channel();
        let dialogs = LinuxDialogs::with_chooser(Arc::new(Held(Mutex::new(closed))));
        let (woken, wake) = mpsc::channel();
        let mut pending = dialogs.begin(
            DialogRequest::Open { multiple: false },
            Arc::new(move || woken.send(()).unwrap()),
        );
        // The event loop keeps running while the dialog is open.
        assert!(pending.try_result().is_none());
        close.send(()).unwrap();
        wake.recv().unwrap();
        assert_eq!(pending.try_result(), Some(Ok(vec![PathBuf::from("/late.txt")])));
    }
}
