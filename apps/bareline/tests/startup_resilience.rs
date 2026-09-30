// SPDX-License-Identifier: MPL-2.0
//! Process-level startup resilience (APP-01, APP-02). A damaged settings file or
//! an unusual portable marker must still open the editor window, and command-line
//! output must reach a redirected handle.
//!
//! Every case copies the built executable into its own temporary directory with
//! a `bareline.portable` marker, points APPDATA, LOCALAPPDATA and TEMP inside
//! that directory as well, and kills every process it starts. The user's profile
//! and any running Bareline are never touched.
//!
//! The cases start GUI processes, so they are ignored by default. Run them with
//! `cargo test -p bareline --test startup_resilience -- --ignored` on an
//! interactive desktop.
#![cfg(windows)]

use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};
use windows::core::BOOL;

const WINDOW_TIMEOUT: Duration = Duration::from_secs(10);
/// winit's default Win32 window class; the editor does not override it.
const EDITOR_WINDOW_CLASS: &str = "Window Class";

struct PortableRoot(PathBuf);
impl PortableRoot {
    fn new(case: &str, marker: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bareline-startup-{case}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        for directory in ["data", "appdata", "localappdata", "temp"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::copy(env!("CARGO_BIN_EXE_bareline"), root.join("bareline.exe")).unwrap();
        fs::write(root.join("bareline.portable"), marker).unwrap();
        Self(root)
    }
    fn settings(&self) -> PathBuf {
        self.0.join("data").join("settings.toml")
    }
    fn data_entries(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.0.join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    /// Output of the process started with `log` (see `command`).
    fn output(&self, log: &str, stream: &str) -> String {
        fs::read_to_string(self.0.join(format!("{log}.{stream}.log"))).unwrap_or_default()
    }
    /// Only the copy inside this temporary portable root is ever started. Each
    /// process gets its own `log` name, so a later one never truncates the output
    /// of one that is still running.
    fn command(&self, args: &[&str], log: &str) -> Command {
        let executable = self.0.join("bareline.exe");
        assert!(executable.starts_with(std::env::temp_dir()));
        assert!(self.0.join("bareline.portable").is_file());
        let mut command = Command::new(executable);
        command
            .args(args)
            .current_dir(&self.0)
            .env("APPDATA", self.0.join("appdata"))
            .env("LOCALAPPDATA", self.0.join("localappdata"))
            .env("TEMP", self.0.join("temp"))
            .env("TMP", self.0.join("temp"))
            .stdin(Stdio::null())
            .stdout(fs::File::create(self.0.join(format!("{log}.stdout.log"))).unwrap())
            .stderr(fs::File::create(self.0.join(format!("{log}.stderr.log"))).unwrap());
        command
    }
}
impl Drop for PortableRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Kills the process when dropped, so a failed assertion never leaks a window.
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Search {
    pid: u32,
    found: bool,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` carries the `Search` owned by `has_editor_window` for the enumeration.
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a live top-level handle supplied by EnumWindows.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != search.pid || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut class = [0u16; 64];
    // SAFETY: `class` is a local buffer of the length passed.
    let length = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    // Only winit's window class is the editor: a startup-failure message box
    // ("#32770") or the console window of a console-subsystem build is not.
    if String::from_utf16_lossy(&class[..length]) == EDITOR_WINDOW_CLASS {
        search.found = true;
        return BOOL(0);
    }
    BOOL(1)
}

fn has_editor_window(pid: u32) -> bool {
    let mut search = Search { pid, found: false };
    // SAFETY: the callback only writes into `search`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// Starts an independent editor and waits for its top-level window; the process
/// is killed when the returned guard drops.
fn assert_window_opens(root: &PortableRoot) -> Running {
    assert_window_opens_with(root, &["--new-instance", "--no-extensions", "--software"])
}

fn assert_window_opens_with(root: &PortableRoot, args: &[&str]) -> Running {
    let mut running = Running(root.command(args, "editor").spawn().unwrap());
    let pid = running.0.id();
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        if has_editor_window(pid) {
            return running;
        }
        if let Some(status) = running.0.try_wait().unwrap() {
            panic!(
                "editor exited with {status} before opening a window: {}",
                root.output("editor", "stderr")
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "no editor window within {WINDOW_TIMEOUT:?}: {}",
                root.output("editor", "stderr")
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_exit(root: &PortableRoot, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let mut running = Running(root.command(args, "cli").spawn().unwrap());
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    let status = loop {
        if let Some(status) = running.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "process did not exit: {args:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    (status, root.output("cli", "stdout"), root.output("cli", "stderr"))
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn malformed_settings_are_quarantined_and_the_window_opens() {
    let root = PortableRoot::new("malformed", b"");
    fs::write(root.settings(), b"[editor\ntab_width = ").unwrap();
    let _running = assert_window_opens(&root);
    assert!(!root.settings().exists());
    let quarantined: Vec<_> = root
        .data_entries()
        .into_iter()
        .filter(|name| name.starts_with("settings.toml.invalid-"))
        .collect();
    assert_eq!(quarantined.len(), 1, "{:?}", root.data_entries());
    assert_eq!(
        fs::read(root.0.join("data").join(&quarantined[0])).unwrap(),
        b"[editor\ntab_width = "
    );
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn large_valid_settings_open_the_window_unchanged() {
    let root = PortableRoot::new("large", b"");
    let mut text = String::from("[editor]\ntab_width = 8\n");
    while text.len() < 70 * 1024 {
        text.push_str("# a long hand-written settings file keeps its comments\n");
    }
    fs::write(root.settings(), &text).unwrap();
    let _running = assert_window_opens(&root);
    assert_eq!(fs::read_to_string(root.settings()).unwrap(), text);
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn utf16_settings_are_converted_with_a_backup_and_the_window_opens() {
    let root = PortableRoot::new("utf16", b"");
    let mut original = vec![0xFF, 0xFE];
    for unit in "[editor]\ntab_width = 8\n".encode_utf16() {
        original.extend(unit.to_le_bytes());
    }
    fs::write(root.settings(), &original).unwrap();
    let _running = assert_window_opens(&root);
    assert_eq!(
        fs::read_to_string(root.settings()).unwrap(),
        "[editor]\ntab_width = 8\n"
    );
    let backups: Vec<_> = root
        .data_entries()
        .into_iter()
        .filter(|name| name.starts_with("settings.toml.utf16-"))
        .collect();
    assert_eq!(backups.len(), 1, "{:?}", root.data_entries());
    assert_eq!(fs::read(root.0.join("data").join(&backups[0])).unwrap(), original);
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn non_empty_portable_marker_still_selects_portable_mode() {
    let root = PortableRoot::new("marker", b"\r\n");
    let _running = assert_window_opens(&root);
    // Portable mode keeps its profile beside the executable.
    assert!(fs::read_dir(root.0.join("localappdata")).unwrap().next().is_none());
    assert!(root.0.join("data").is_dir());
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn help_version_and_argument_errors_reach_redirected_output() {
    let root = PortableRoot::new("cli", b"");
    let (status, stdout, _) = wait_for_exit(&root, &["--help"]);
    assert!(status.success());
    for option in ["--software", "--hardware", "--help", "--version", "--line"] {
        assert!(stdout.contains(option), "{option} missing from help: {stdout}");
    }
    assert!(!stdout.contains("--diagnostic-root"));

    let (status, stdout, _) = wait_for_exit(&root, &["--version"]);
    assert!(status.success());
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));

    let (status, _, stderr) = wait_for_exit(&root, &["--line", "0"]);
    assert!(!status.success());
    assert!(stderr.contains("bareline: ") && stderr.contains("Usage:"), "{stderr}");
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn forwarding_launch_leaves_the_running_instances_settings_file_alone() {
    let root = PortableRoot::new("forward", b"");
    // Without --new-instance, --no-extensions or --no-session this becomes the
    // primary instance for the portable profile.
    let _primary = assert_window_opens_with(&root, &["--software"]);
    // The user saves a mistake in the settings file the primary has open.
    fs::write(root.settings(), b"[editor\ntab_width = ").unwrap();
    let document = root.0.join("forwarded.txt");
    fs::write(&document, "forwarded\n").unwrap();
    let (status, _, stderr) = wait_for_exit(&root, &[document.to_str().unwrap()]);
    assert!(status.success(), "forwarding launch failed: {stderr}");
    assert_eq!(fs::read(root.settings()).unwrap(), b"[editor\ntab_width = ");
    assert!(
        !root
            .data_entries()
            .iter()
            .any(|name| name.starts_with("settings.toml.invalid-") || name.starts_with("settings.toml.utf16-")),
        "{:?}",
        root.data_entries()
    );
}

#[test]
#[ignore = "starts GUI editor processes; run with --ignored on an interactive desktop"]
fn drive_relative_command_line_path_opens_without_a_rejection() {
    let root = PortableRoot::new("drive-relative", b"");
    let text = root.0.to_str().unwrap().to_owned();
    // Only meaningful when the temporary directory is on a lettered drive.
    let Some(drive) = text.get(..2).filter(|drive| drive.ends_with(':')) else {
        return;
    };
    fs::write(root.0.join("foo.txt"), "drive relative\n").unwrap();
    // "C:foo.txt" names foo.txt in the current directory of drive C:, which is
    // the portable root the process starts in.
    let argument = format!("{drive}foo.txt");
    let _running = assert_window_opens_with(
        &root,
        &["--new-instance", "--no-extensions", "--software", argument.as_str()],
    );
    let stderr = root.output("editor", "stderr");
    assert!(!stderr.contains("startup:rejected-paths"), "{stderr}");
}
