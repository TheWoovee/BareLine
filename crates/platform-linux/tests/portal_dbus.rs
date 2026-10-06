// SPDX-License-Identifier: MPL-2.0
#![cfg(target_os = "linux")]
//! The portal client over real D-Bus: a private bus (`dbus-daemon`) with a fake
//! `org.freedesktop.portal.Desktop` that answers FileChooser requests through
//! the Request `Response` signal and Settings reads and changes, as the real
//! portal does. Ignored by default because it starts `dbus-daemon`:
//!
//! ```text
//! cargo test -p bareline-platform-linux --test portal_dbus -- --ignored
//! ```
use bareline_platform::{PlatformServices, SaveDialogOptions, SaveFileKind};
use bareline_platform_linux::{
    LinuxAppearance, LinuxDialogs,
    dialogs::DialogRequest,
    portal::{DesktopPortal, PortalError},
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const OBJECT: &str = "/org/freedesktop/portal/desktop";

/// A private session bus, stopped when dropped.
struct Bus {
    daemon: Child,
    address: String,
}
impl Bus {
    fn start() -> Self {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon is needed for this test");
        let mut address = String::new();
        BufReader::new(daemon.stdout.as_mut().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            daemon,
            address: address.trim().to_owned(),
        }
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// What one FileChooser request carried, as the fake portal saw it.
#[derive(Clone, Debug, Default)]
struct Asked {
    method: String,
    title: String,
    multiple: Option<bool>,
    directory: Option<bool>,
    filters: Vec<(String, Vec<(u32, String)>)>,
    current_name: Option<String>,
    current_folder: Option<Vec<u8>>,
}
/// How the fake answers: a response code and URIs.
#[derive(Clone)]
struct Answer {
    code: u32,
    uris: Vec<String>,
}
struct FakeChooser {
    asked: Arc<Mutex<Vec<Asked>>>,
    answer: Arc<Mutex<Answer>>,
}
fn boolean(options: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    match options.get(key).map(|value| &**value) {
        Some(Value::Bool(value)) => Some(*value),
        _ => None,
    }
}
fn text(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match options.get(key).map(|value| &**value) {
        Some(Value::Str(value)) => Some(value.to_string()),
        _ => None,
    }
}
/// `a(sa(us))` as plain data.
fn fields<'v, 'a>(value: &'v Value<'a>) -> &'v [Value<'a>] {
    match value {
        Value::Structure(structure) => structure.fields(),
        _ => &[],
    }
}
fn filters(value: &Value<'_>) -> Vec<(String, Vec<(u32, String)>)> {
    let Value::Array(list) = value else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|filter| match fields(filter) {
            [Value::Str(label), Value::Array(patterns)] => Some((
                label.to_string(),
                patterns
                    .iter()
                    .filter_map(|pattern| match fields(pattern) {
                        [Value::U32(kind), Value::Str(glob)] => Some((*kind, glob.to_string())),
                        _ => None,
                    })
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}
/// `ay` as bytes.
fn bytes(value: &Value<'_>) -> Vec<u8> {
    match value {
        Value::Array(list) => list
            .iter()
            .filter_map(|byte| match byte {
                Value::U8(byte) => Some(*byte),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
impl FakeChooser {
    fn respond(
        &self,
        method: &str,
        connection: &zbus::Connection,
        header: &zbus::message::Header<'_>,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let token =
            text(&options, "handle_token").ok_or_else(|| zbus::fdo::Error::InvalidArgs("handle_token".into()))?;
        let sender = header
            .sender()
            .map(|sender| sender.to_string())
            .ok_or_else(|| zbus::fdo::Error::Failed("no sender".into()))?;
        let filters = options.get("filters").map(|value| filters(value)).unwrap_or_default();
        let current_folder = options.get("current_folder").map(|value| bytes(value));
        self.asked.lock().unwrap().push(Asked {
            method: method.into(),
            title: title.into(),
            multiple: boolean(&options, "multiple"),
            directory: boolean(&options, "directory"),
            filters,
            current_name: text(&options, "current_name"),
            current_folder,
        });
        let path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{token}",
            sender.trim_start_matches(':').replace('.', "_")
        );
        let answer = self.answer.lock().unwrap().clone();
        let bus = zbus::blocking::Connection::from(connection.clone());
        let request = path.clone();
        // The answer comes later, as a signal, after the user closed the dialog.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let mut results: HashMap<&str, Value<'_>> = HashMap::new();
            results.insert("uris", Value::from(answer.uris));
            bus.emit_signal(
                Some(sender.as_str()),
                request.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(answer.code, results),
            )
            .unwrap();
        });
        OwnedObjectPath::try_from(path).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }
}
#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl FakeChooser {
    fn open_file(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.respond("OpenFile", connection, &header, title, options)
    }
    fn save_file(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.respond("SaveFile", connection, &header, title, options)
    }
}
fn owned(value: u32) -> zbus::fdo::Result<OwnedValue> {
    OwnedValue::try_from(Value::from(value)).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
}
struct FakeSettings {
    scheme: Arc<Mutex<u32>>,
}
#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl FakeSettings {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        match (namespace, key) {
            ("org.freedesktop.appearance", "color-scheme") => owned(*self.scheme.lock().unwrap()),
            ("org.freedesktop.appearance", "contrast") => owned(0),
            _ => Err(zbus::fdo::Error::Failed("org.freedesktop.portal.Error.NotFound".into())),
        }
    }
}

struct Portal {
    bus: Bus,
    service: zbus::blocking::Connection,
    asked: Arc<Mutex<Vec<Asked>>>,
    answer: Arc<Mutex<Answer>>,
    scheme: Arc<Mutex<u32>>,
}
impl Portal {
    fn start() -> Self {
        let bus = Bus::start();
        let asked = Arc::default();
        let answer = Arc::new(Mutex::new(Answer {
            code: 0,
            uris: Vec::new(),
        }));
        let scheme = Arc::new(Mutex::new(1));
        let service = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.portal.Desktop")
            .unwrap()
            .serve_at(
                OBJECT,
                FakeChooser {
                    asked: Arc::clone(&asked),
                    answer: Arc::clone(&answer),
                },
            )
            .unwrap()
            .serve_at(
                OBJECT,
                FakeSettings {
                    scheme: Arc::clone(&scheme),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        Self {
            bus,
            service,
            asked,
            answer,
            scheme,
        }
    }
    fn answer(&self, code: u32, uris: &[&str]) {
        *self.answer.lock().unwrap() = Answer {
            code,
            uris: uris.iter().map(|uri| uri.to_string()).collect(),
        };
    }
    fn last(&self) -> Asked {
        self.asked.lock().unwrap().last().cloned().unwrap()
    }
    fn client(&self) -> DesktopPortal {
        DesktopPortal::at(&self.bus.address).unwrap()
    }
}

#[test]
#[ignore = "starts a private dbus-daemon; run with --ignored where dbus-daemon is installed"]
fn file_chooser_requests_cross_the_bus_and_answers_come_back_as_signals() {
    let portal = Portal::start();
    let dialogs = LinuxDialogs::with_chooser(portal.client().file_chooser());
    portal.answer(0, &["file:///tmp/picked%20one.txt", "file:///tmp/two.md"]);
    assert_eq!(
        dialogs.open_files().unwrap(),
        vec![PathBuf::from("/tmp/picked one.txt"), PathBuf::from("/tmp/two.md")]
    );
    let asked = portal.last();
    assert_eq!((asked.method.as_str(), asked.title.as_str()), ("OpenFile", "Open"));
    assert_eq!((asked.multiple, asked.directory), (Some(true), Some(false)));
    assert_eq!(asked.filters[0].0, "Text files");
    assert_eq!(asked.filters[1], ("All files".to_owned(), vec![(0, "*".to_owned())]));

    portal.answer(0, &["file:///home/u/report"]);
    let options = SaveDialogOptions::new(SaveFileKind::Html)
        .named("report.html")
        .in_directory(Some(PathBuf::from("/home/u")));
    assert_eq!(
        dialogs.save_file_with(&options).unwrap(),
        Some(PathBuf::from("/home/u/report.html"))
    );
    let asked = portal.last();
    assert_eq!(asked.method, "SaveFile");
    assert_eq!(asked.current_name.as_deref(), Some("report.html"));
    assert_eq!(asked.current_folder.as_deref(), Some(&b"/home/u\0"[..]));

    portal.answer(0, &["file:///home/u/project"]);
    assert_eq!(dialogs.pick_folder().unwrap(), Some(PathBuf::from("/home/u/project")));
    assert_eq!(portal.last().directory, Some(true));

    // The user cancels: no path and no error.
    portal.answer(1, &[]);
    assert_eq!(dialogs.open_file().unwrap(), None);

    // The event loop is never blocked: the request runs on its own worker.
    portal.answer(0, &["file:///tmp/later.txt"]);
    let (woken, wake) = mpsc::channel();
    let mut pending = dialogs.begin(
        DialogRequest::Open { multiple: false },
        Arc::new(move || woken.send(()).unwrap()),
    );
    wake.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(pending.try_result(), Some(Ok(vec![PathBuf::from("/tmp/later.txt")])));
}

#[test]
#[ignore = "starts a private dbus-daemon; run with --ignored where dbus-daemon is installed"]
fn appearance_is_read_and_followed_over_the_bus() {
    let portal = Portal::start();
    let settings = portal.client().settings();
    assert_eq!(settings.read("org.freedesktop.appearance", "color-scheme"), Ok(Some(1)));
    assert_eq!(settings.read("org.freedesktop.appearance", "unknown"), Ok(None));
    let (notified, notifications) = mpsc::channel();
    let appearance = LinuxAppearance::with_settings(
        settings,
        Arc::new(move || {
            let _ = notified.send(());
        }),
    );
    notifications.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(appearance.dark(), Some(true));
    assert!(!appearance.high_contrast());
    // The desktop switches to light: the change arrives as a signal.
    *portal.scheme.lock().unwrap() = 2;
    let deadline = Instant::now() + Duration::from_secs(20);
    while appearance.dark() != Some(false) {
        assert!(Instant::now() < deadline, "the change never arrived");
        portal
            .service
            .emit_signal(
                None::<&str>,
                OBJECT,
                "org.freedesktop.portal.Settings",
                "SettingChanged",
                &("org.freedesktop.appearance", "color-scheme", Value::from(2u32)),
            )
            .unwrap();
        let _ = notifications.recv_timeout(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "starts a private dbus-daemon; run with --ignored where dbus-daemon is installed"]
fn a_bus_without_a_portal_makes_dialogs_unsupported() {
    let bus = Bus::start();
    let portal = DesktopPortal::at(&bus.address).unwrap();
    let dialogs = LinuxDialogs::with_chooser(portal.file_chooser());
    let error = dialogs.open_file().unwrap_err();
    assert!(
        error.starts_with("This system does not support the Open dialog"),
        "{error}"
    );
    assert!(matches!(
        portal.settings().read("org.freedesktop.appearance", "color-scheme"),
        Err(PortalError::Unavailable(_))
    ));
}
