// SPDX-License-Identifier: MPL-2.0
//! The XDG desktop portal on the session bus (`org.freedesktop.portal.Desktop`),
//! through `zbus`'s blocking API: the FileChooser for dialogs and the Settings
//! interface for the appearance. Everything here blocks its caller until the
//! portal answers, so callers run it on a worker thread, never on the event loop.
//!
//! The dialog and appearance logic talks to the traits [`FileChooser`] and
//! [`Settings`], so tests drive it through fakes; [`DesktopPortal`] is the real
//! D-Bus implementation.
use std::{collections::HashMap, fmt, path::Path, sync::Arc};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";

/// Why a portal call did not produce an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortalError {
    /// No session bus, no portal service, or a portal without this interface:
    /// the feature is unsupported here, which callers report as such.
    Unavailable(String),
    /// The portal exists but the call failed.
    Failed(String),
}
impl fmt::Display for PortalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "the desktop portal is not available ({reason})"),
            Self::Failed(reason) => write!(f, "the desktop portal failed ({reason})"),
        }
    }
}
impl std::error::Error for PortalError {}
fn classify(error: zbus::Error) -> PortalError {
    let text = error.to_string();
    let unavailable = [
        "ServiceUnknown",
        "NameHasNoOwner",
        "UnknownMethod",
        "UnknownInterface",
        "UnknownObject",
        "NotSupported",
    ];
    if unavailable.iter().any(|name| text.contains(name)) {
        PortalError::Unavailable(text)
    } else {
        PortalError::Failed(text)
    }
}

/// One `(label, [(kind, pattern)])` filter of the FileChooser; kind 0 is a glob
/// and 1 a MIME type.
pub type Filter = (String, Vec<(u32, String)>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserMethod {
    OpenFile,
    SaveFile,
}
/// The FileChooser options Bareline uses, before they become an `a{sv}`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChooserOptions {
    pub accept_label: Option<String>,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<Filter>,
    pub current_filter: Option<Filter>,
    pub current_name: Option<String>,
    /// A folder, as raw bytes (the portal takes `ay`).
    pub current_folder: Option<Vec<u8>>,
}
/// The `Response` signal of a FileChooser request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChooserResponse {
    /// 0 chosen, 1 cancelled by the user, 2 ended another way.
    pub code: u32,
    pub uris: Vec<String>,
}
/// `org.freedesktop.portal.FileChooser`.
pub trait FileChooser: Send + Sync {
    /// Shows the dialog and blocks until it closes.
    fn choose(
        &self,
        method: ChooserMethod,
        parent: &str,
        title: &str,
        options: &ChooserOptions,
    ) -> Result<ChooserResponse, PortalError>;
}
/// `org.freedesktop.portal.Settings`, the values Bareline reads as integers.
pub trait Settings: Send + Sync {
    /// The current value of `namespace`/`key`; `None` when the portal does not know it.
    fn read(&self, namespace: &str, key: &str) -> Result<Option<u32>, PortalError>;
    /// Blocks, calling `changed(namespace, key, value)` for every integer setting
    /// that changes, until `changed` returns false or the connection ends.
    fn watch(&self, changed: &mut dyn FnMut(&str, &str, u32) -> bool) -> Result<(), PortalError>;
}

/// An integer from a portal value, also when it arrives wrapped in a variant
/// (the deprecated `Read` method wraps it twice).
fn integer(value: &Value<'_>) -> Option<u32> {
    match value {
        Value::U32(number) => Some(*number),
        Value::Value(inner) => integer(inner),
        _ => None,
    }
}
fn strings(value: &Value<'_>) -> Vec<String> {
    match value {
        Value::Value(inner) => strings(inner),
        Value::Array(array) => array
            .iter()
            .filter_map(|item| match item {
                Value::Str(text) => Some(text.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
/// The request object path the portal will use for `token` (portal API 0.9+).
fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}
fn token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or_default();
    format!("bareline_{}_{nanos}", std::process::id())
}

/// The portal of this session.
#[derive(Clone)]
pub struct DesktopPortal {
    connection: Connection,
}
impl DesktopPortal {
    /// Connects to the session bus. `Unavailable` without one.
    pub fn session() -> Result<Self, PortalError> {
        Connection::session()
            .map(|connection| Self { connection })
            .map_err(|error| PortalError::Unavailable(error.to_string()))
    }
    /// A portal on an explicit bus address (tests run their own bus).
    pub fn at(address: &str) -> Result<Self, PortalError> {
        zbus::blocking::connection::Builder::address(address)
            .and_then(|builder| builder.build())
            .map(|connection| Self { connection })
            .map_err(|error| PortalError::Unavailable(error.to_string()))
    }
    fn proxy(&self, path: &str, interface: &'static str) -> Result<Proxy<'static>, PortalError> {
        Proxy::new(&self.connection, DESTINATION, path.to_owned(), interface).map_err(classify)
    }
    pub fn file_chooser(&self) -> Arc<dyn FileChooser> {
        Arc::new(self.clone())
    }
    pub fn settings(&self) -> Arc<dyn Settings> {
        Arc::new(self.clone())
    }
}
impl FileChooser for DesktopPortal {
    fn choose(
        &self,
        method: ChooserMethod,
        parent: &str,
        title: &str,
        options: &ChooserOptions,
    ) -> Result<ChooserResponse, PortalError> {
        let token = token();
        let unique = self
            .connection
            .unique_name()
            .map(|name| name.to_string())
            .ok_or_else(|| PortalError::Failed("no unique bus name".into()))?;
        let expected = request_path(&unique, &token);
        // Subscribe before calling, so a fast answer is not missed.
        let request = self.proxy(&expected, "org.freedesktop.portal.Request")?;
        let mut responses = request.receive_signal("Response").map_err(classify)?;
        let mut map: HashMap<&str, Value<'_>> = HashMap::new();
        map.insert("handle_token", Value::from(token.as_str()));
        map.insert("modal", Value::from(true));
        if let Some(label) = &options.accept_label {
            map.insert("accept_label", Value::from(label.as_str()));
        }
        if method == ChooserMethod::OpenFile {
            map.insert("multiple", Value::from(options.multiple));
            map.insert("directory", Value::from(options.directory));
        }
        if !options.filters.is_empty() {
            map.insert("filters", Value::from(options.filters.clone()));
        }
        if let Some(current) = &options.current_filter {
            map.insert("current_filter", Value::from(current.clone()));
        }
        if let Some(name) = &options.current_name {
            map.insert("current_name", Value::from(name.as_str()));
        }
        if let Some(folder) = &options.current_folder {
            // A NUL-terminated byte string, so non-UTF-8 folder names survive.
            let mut bytes = folder.clone();
            bytes.push(0);
            map.insert("current_folder", Value::from(bytes));
        }
        let chooser = self.proxy(OBJECT, "org.freedesktop.portal.FileChooser")?;
        let member = match method {
            ChooserMethod::OpenFile => "OpenFile",
            ChooserMethod::SaveFile => "SaveFile",
        };
        let handle: OwnedObjectPath = chooser.call(member, &(parent, title, map)).map_err(classify)?;
        // Portals before 0.9 choose their own path; listen there instead.
        let mut fallback;
        let responses: &mut dyn Iterator<Item = zbus::Message> = if handle.as_str() == expected {
            &mut responses
        } else {
            let request = self.proxy(handle.as_str(), "org.freedesktop.portal.Request")?;
            fallback = request.receive_signal("Response").map_err(classify)?;
            &mut fallback
        };
        let message = responses
            .next()
            .ok_or_else(|| PortalError::Failed("the portal closed the request".into()))?;
        let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(classify)?;
        Ok(ChooserResponse {
            code,
            uris: results.get("uris").map(|value| strings(value)).unwrap_or_default(),
        })
    }
}
impl Settings for DesktopPortal {
    fn read(&self, namespace: &str, key: &str) -> Result<Option<u32>, PortalError> {
        let settings = self.proxy(OBJECT, "org.freedesktop.portal.Settings")?;
        let value: Result<OwnedValue, _> = settings.call("ReadOne", &(namespace, key));
        let value = match value {
            Ok(value) => value,
            // Settings version 1 has only the deprecated `Read`.
            Err(error) if error.to_string().contains("UnknownMethod") => {
                settings.call("Read", &(namespace, key)).map_err(classify)?
            }
            Err(error) if error.to_string().contains("NotFound") => return Ok(None),
            Err(error) => return Err(classify(error)),
        };
        Ok(integer(&value))
    }
    fn watch(&self, changed: &mut dyn FnMut(&str, &str, u32) -> bool) -> Result<(), PortalError> {
        let settings = self.proxy(OBJECT, "org.freedesktop.portal.Settings")?;
        for message in settings.receive_signal("SettingChanged").map_err(classify)? {
            let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedValue)>() else {
                continue;
            };
            if let Some(value) = integer(&value)
                && !changed(&namespace, &key, value)
            {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// A local path from a portal `file://` URI. Other schemes are not local files.
pub fn path_from_uri(uri: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri.strip_prefix("file://")?;
    // An empty or `localhost` authority means this machine.
    let path = if rest.starts_with('/') {
        rest
    } else {
        rest.strip_prefix("localhost")?
    };
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    if decoded.contains(&0) || !decoded.starts_with(b"/") {
        return None;
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}
/// The FileChooser's `parent_window` for an X11 window; Wayland parents need an
/// exported handle, so they pass an empty string (the dialog is then not
/// attached to the editor window).
pub fn x11_parent(window: u64) -> String {
    format!("x11:{window:x}")
}
/// `current_folder` bytes for a folder.
pub fn folder_bytes(folder: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    folder.as_os_str().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_follow_the_portal_rule() {
        assert_eq!(
            request_path(":1.42", "bareline_7"),
            "/org/freedesktop/portal/desktop/request/1_42/bareline_7"
        );
    }
    #[test]
    fn file_uris_become_byte_exact_local_paths() {
        assert_eq!(
            path_from_uri("file:///home/u/a%20b.txt").unwrap(),
            Path::new("/home/u/a b.txt")
        );
        assert_eq!(path_from_uri("file://localhost/tmp/x").unwrap(), Path::new("/tmp/x"));
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            path_from_uri("file:///tmp/caf%C3%A9%FF")
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"/tmp/caf\xc3\xa9\xff"
        );
        for refused in [
            "https://example.com/a",
            "file://server/share/a",
            "file:///a%00b",
            "file:///a%zz",
            "file:///a%2",
        ] {
            assert_eq!(path_from_uri(refused), None, "{refused}");
        }
    }
    #[test]
    fn values_unwrap_nested_variants() {
        assert_eq!(integer(&Value::from(1u32)), Some(1));
        assert_eq!(integer(&Value::Value(Box::new(Value::from(2u32)))), Some(2));
        assert_eq!(integer(&Value::from("dark")), None);
        assert_eq!(
            strings(&Value::from(vec!["file:///a".to_owned()])),
            vec!["file:///a".to_owned()]
        );
        assert_eq!(x11_parent(0x2a00007), "x11:2a00007");
    }
    #[test]
    fn missing_services_are_unavailable_not_failures() {
        let error = zbus::Error::Failure("org.freedesktop.DBus.Error.ServiceUnknown: no portal".into());
        assert!(matches!(classify(error), PortalError::Unavailable(_)));
        assert!(matches!(
            classify(zbus::Error::Failure("broken".into())),
            PortalError::Failed(_)
        ));
    }
}
