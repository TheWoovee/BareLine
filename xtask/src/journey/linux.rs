// SPDX-License-Identifier: MPL-2.0
//! The X11 desktop of the Linux journeys: xdotool finds windows by their
//! _NET_WM_PID, focuses them and types through XTEST; ImageMagick's `import`
//! captures a window as PPM. Winit uses X11 where these tools see it.
//!
//! Each journey attempt owns its display: an Xvfb server (no window manager)
//! on a display the harness found free, which reports through `-displayfd`
//! when it accepts clients (a display another server took in the meantime
//! makes it exit at once, and the next free one is tried, where `xvfb-run -a`
//! races), and runs with `-noreset`, so it does not reset, and briefly refuse
//! connections, whenever its last client disconnects between an editor exit
//! and the next launch. Beside it runs a private session bus
//! with the harness's portal stand-in (tests/e2e/fake_portal.py), which
//! answers the editor's file choosers with the paths the journey stages.
//! `BARELINE_QA_DISPLAY=inherit` uses the caller's DISPLAY and session bus
//! instead (a real portal's chooser is then driven by typing its location).

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::Image;
use super::ordinary::{Desktop, Window, output_within};
use super::steps::Failure;

const TOOL_DEADLINE: Duration = Duration::from_secs(20);
/// Pacing between typed characters. A character outside the keymap makes
/// xdotool rebind a spare keycode; winit must see that mapping change before
/// the key event, which takes far longer than an ASCII keystroke.
const ASCII_DELAY_MS: &str = "12";
const UNICODE_DELAY_MS: &str = "90";
/// The private server's arguments after its display: it writes the display
/// on standard output once it accepts clients, never resets, and listens on
/// its local sockets only.
const XVFB_ARGUMENTS: [&str; 8] = [
    "-displayfd",
    "1",
    "-screen",
    "0",
    "1600x1000x24",
    "-noreset",
    "-nolisten",
    "tcp",
];
const PORTAL: &str = "org.freedesktop.portal.Desktop";

pub(super) fn desktop(root: &Path, scratch: &Path) -> Result<Box<dyn Desktop>, Failure> {
    let desktop = X11::start(root, scratch)?;
    let xdotool = desktop.tool(&["xdotool", "version"])?;
    if !xdotool.status.success() {
        return Err(Failure::environment("xdotool is not usable in this session"));
    }
    desktop.tool(&["import", "-version"])?;
    Ok(Box::new(desktop))
}

struct X11 {
    /// The DISPLAY every tool and editor of this desktop uses.
    display: OsString,
    /// The private X server, ended with the desktop.
    server: Option<Child>,
    /// The private session bus with the portal stand-in, or why there is none.
    bus: Result<PrivateBus, String>,
}

/// A session bus of the attempt's own, with the harness's portal stand-in.
struct PrivateBus {
    address: String,
    daemon: Child,
    portal: Child,
    /// Where the next chooser's paths are staged.
    answers: PathBuf,
    /// One JSON line per chooser request the stand-in answered.
    requests: PathBuf,
}

impl Drop for X11 {
    fn drop(&mut self) {
        let mut children: Vec<&mut Child> = Vec::new();
        if let Ok(bus) = &mut self.bus {
            children.push(&mut bus.portal);
            children.push(&mut bus.daemon);
        }
        children.extend(self.server.as_mut());
        for child in children {
            terminate(child);
        }
    }
}

/// End a helper with SIGTERM, so Xvfb removes its lock file and socket, and
/// kill it when it has not exited after two seconds.
fn terminate(child: &mut Child) {
    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Read the first line a child writes on its standard output within
/// `deadline`, on a thread, so a child that never writes cannot hold the run.
fn first_line(child: &mut Child, deadline: Duration) -> Option<String> {
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    receiver
        .recv_timeout(deadline)
        .ok()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
}

/// A log file for a helper's diagnostics in the attempt's scratch.
fn log_file(scratch: &Path, name: &str) -> Stdio {
    std::fs::File::create(scratch.join(name)).map_or_else(|_| Stdio::null(), Stdio::from)
}

/// Whether an editor's standard error shows the X server refusing its
/// connection: the display's failure, never the product's.
pub(super) fn x_connection_refused(stderr: &str) -> bool {
    stderr.contains("Failed to open connection to X server")
}

impl X11 {
    fn start(root: &Path, scratch: &Path) -> Result<Self, Failure> {
        if std::env::var_os("BARELINE_QA_DISPLAY").is_some_and(|mode| mode == "inherit") {
            let display = std::env::var_os("DISPLAY")
                .ok_or_else(|| Failure::environment("BARELINE_QA_DISPLAY=inherit without an X11 DISPLAY"))?;
            return Ok(Self {
                display,
                server: None,
                bus: Err("the caller's session bus is used (BARELINE_QA_DISPLAY=inherit)".into()),
            });
        }
        let (server, display) = start_xvfb(scratch)?;
        let mut desktop = Self {
            display,
            server: Some(server),
            bus: Err(String::new()),
        };
        desktop.bus = if std::env::var_os("BARELINE_QA_PORTAL").is_some_and(|mode| mode == "none") {
            Err("the portal stand-in was turned off (BARELINE_QA_PORTAL=none)".into())
        } else {
            start_bus(root, scratch)
        };
        Ok(desktop)
    }

    /// Run a tool on this desktop's display with a deadline; a missing tool
    /// is an environment failure.
    fn tool(&self, arguments: &[&str]) -> Result<Output, Failure> {
        output_within(
            Command::new(arguments[0])
                .args(&arguments[1..])
                .env("DISPLAY", &self.display),
            arguments[0],
            TOOL_DEADLINE,
        )
    }

    fn checked(&self, arguments: &[&str]) -> Result<String, Failure> {
        let output = self.tool(arguments)?;
        if !output.status.success() {
            return Err(Failure::harness(format!(
                "{} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Window ids from `xdotool search`; exit status 1 means none matched.
    fn search(&self, arguments: &[&str]) -> Result<Vec<Window>, Failure> {
        let output = self.tool(arguments)?;
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(Failure::harness(format!(
                "xdotool search failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        // A window can disappear between the search and the geometry query.
        Ok(ids.iter().filter_map(|id| self.describe_window(id).ok()).collect())
    }

    fn describe_window(&self, id: &str) -> Result<Window, Failure> {
        let geometry = self.checked(&["xdotool", "getwindowgeometry", "--shell", id])?;
        let value = |name: &str| {
            geometry
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
                .and_then(|value| value.trim().parse::<i32>().ok())
                .unwrap_or(0)
        };
        let title = self
            .checked(&["xdotool", "getwindowname", id])
            .ok()
            .map(|title| title.trim().to_owned());
        let pid = self
            .checked(&["xdotool", "getwindowpid", id])
            .ok()
            .and_then(|pid| pid.trim().parse().ok());
        Ok(Window {
            id: id.to_owned(),
            pid,
            x: value("X"),
            y: value("Y"),
            width: value("WIDTH"),
            height: value("HEIGHT"),
            title,
        })
    }
}

/// Start a private Xvfb on a free display. Each candidate display without a
/// lock file or a socket (a file below /tmp/.X11-unix, or the abstract
/// socket Linux clients try first) is tried in turn; a server whose display
/// was taken in the meantime cannot listen and exits at once, and the next
/// candidate is tried. The server writes its display once it accepts clients.
fn start_xvfb(scratch: &Path) -> Result<(Child, OsString), Failure> {
    // Start where another run is unlikely to: runs started together differ in pid.
    let first = 100 + std::process::id() % 400;
    for number in (first..first + 64).filter(|number| !display_in_use(*number)) {
        let mut server = Command::new("Xvfb")
            .arg(format!(":{number}"))
            .args(XVFB_ARGUMENTS)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log_file(scratch, "xvfb.log"))
            .spawn()
            .map_err(|error| Failure::environment(format!("Xvfb is not available: {error}")))?;
        if first_line(&mut server, Duration::from_secs(15)) == Some(number.to_string()) && display_in_use(number) {
            return Ok((server, format!(":{number}").into()));
        }
        terminate(&mut server);
    }
    Err(Failure::environment(format!(
        "Xvfb could not start on any free display from :{first} (see xvfb.log in the scratch)"
    )))
}

/// Whether X display `number` is taken: its lock file, its socket file or
/// its abstract socket exists.
fn display_in_use(number: u32) -> bool {
    let socket = format!("/tmp/.X11-unix/X{number}");
    Path::new(&format!("/tmp/.X{number}-lock")).exists()
        || Path::new(&socket).exists()
        || std::fs::read_to_string("/proc/net/unix").is_ok_and(|table| {
            table
                .lines()
                .any(|line| line.split_whitespace().last() == Some(format!("@{socket}").as_str()))
        })
}

/// Start a session bus of the attempt's own (no activatable services, so
/// nothing but the stand-in can answer) and the portal stand-in on it, and
/// wait until the stand-in owns the portal name.
fn start_bus(root: &Path, scratch: &Path) -> Result<PrivateBus, String> {
    let config = scratch.join("session-bus.conf");
    std::fs::write(
        &config,
        "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
         <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
         <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>\n",
    )
    .map_err(|error| format!("session bus configuration: {error}"))?;
    let mut daemon = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--nopidfile", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log_file(scratch, "session-bus.log"))
        .spawn()
        .map_err(|error| format!("dbus-daemon is not available: {error}"))?;
    let Some(address) = first_line(&mut daemon, Duration::from_secs(10)) else {
        let _ = daemon.kill();
        let _ = daemon.wait();
        return Err("dbus-daemon printed no address within 10 s".into());
    };
    // The stand-in needs PyGObject, which the system interpreter carries;
    // CI's setup-python interpreter first on PATH does not.
    let python = std::env::var_os("BARELINE_QA_PORTAL_PYTHON").unwrap_or_else(|| {
        if Path::new("/usr/bin/python3").exists() {
            "/usr/bin/python3".into()
        } else {
            "python3".into()
        }
    });
    let answers = scratch.join("portal-answers.txt");
    let requests = scratch.join("portal-requests.jsonl");
    let portal = Command::new(&python)
        .arg(root.join("tests/e2e/fake_portal.py"))
        .env("DBUS_SESSION_BUS_ADDRESS", &address)
        .env("FAKE_PORTAL_ANSWERS", &answers)
        .env("FAKE_PORTAL_LOG", &requests)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(log_file(scratch, "portal.log"))
        .stderr(log_file(scratch, "portal.stderr.log"))
        .spawn();
    let mut portal = match portal {
        Ok(portal) => portal,
        Err(error) => {
            let _ = daemon.kill();
            let _ = daemon.wait();
            return Err(format!("the portal stand-in could not start: {error}"));
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if bus_names(&address, "ListNames").contains(PORTAL) {
            return Ok(PrivateBus {
                address,
                daemon,
                portal,
                answers,
                requests,
            });
        }
        let exited = matches!(portal.try_wait(), Ok(Some(_)));
        if exited || Instant::now() >= deadline {
            let _ = portal.kill();
            let _ = portal.wait();
            let _ = daemon.kill();
            let _ = daemon.wait();
            let errors = std::fs::read_to_string(scratch.join("portal.stderr.log")).unwrap_or_default();
            let last = errors.lines().last().unwrap_or("no output");
            return Err(format!(
                "the portal stand-in did not take {PORTAL} on the private bus (needs PyGObject): {last}"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The names `method` (ListNames or ListActivatableNames) lists on `bus`;
/// dbus-send waits up to the D-Bus default (about 25 s) for a reply, so it
/// runs under its own deadline.
fn bus_names(bus: impl AsRef<std::ffi::OsStr>, method: &str) -> String {
    output_within(
        Command::new("dbus-send").env("DBUS_SESSION_BUS_ADDRESS", bus).args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            &format!("org.freedesktop.DBus.{method}"),
        ]),
        "dbus-send",
        Duration::from_secs(8),
    )
    .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    .unwrap_or_default()
}

/// xdotool's key name for one neutral chord (`Primary+Shift+P` is `ctrl+shift+p`).
pub(super) fn x11_chord(chord: &str) -> Result<String, Failure> {
    chord
        .split('+')
        .map(|part| {
            Ok(match part {
                "Primary" => "ctrl".to_owned(),
                "Shift" => "shift".to_owned(),
                "Alt" => "alt".to_owned(),
                "Return" | "Escape" | "Tab" | "Home" | "End" | "Up" | "Down" | "Left" | "Right" => part.to_owned(),
                "PageDown" => "Next".to_owned(),
                "Space" => "space".to_owned(),
                key if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric()) => key.to_ascii_lowercase(),
                key if key
                    .strip_prefix('F')
                    .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n))) =>
                {
                    key.to_owned()
                }
                other => return Err(Failure::harness(format!("unknown key {other:?} in chord {chord:?}"))),
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("+"))
}

/// Split text into runs typed with one pacing each: ASCII fast, the rest slow.
pub(super) fn typing_runs(text: &str) -> Vec<(&str, bool)> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut ascii = None;
    for (index, character) in text.char_indices() {
        let current = character.is_ascii();
        if ascii.is_some_and(|previous| previous != current) {
            runs.push((&text[start..index], ascii.unwrap_or(true)));
            start = index;
        }
        ascii = Some(current);
    }
    if start < text.len() {
        runs.push((&text[start..], ascii.unwrap_or(true)));
    }
    runs
}

/// A binary PPM (`P6`, 8-bit) as a top-down BGRA image.
pub(super) fn parse_ppm(bytes: &[u8]) -> Result<Image, String> {
    let mut fields = Vec::new();
    let mut index = 0;
    while fields.len() < 4 {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) == Some(&b'#') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if start == index {
            return Err("truncated PPM header".into());
        }
        fields.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
    }
    // Exactly one whitespace byte separates the header from the pixels.
    index += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        return Err(format!("unsupported PPM {} with maximum {}", fields[0], fields[3]));
    }
    let width: i32 = fields[1].parse().map_err(|_| "invalid PPM width")?;
    let height: i32 = fields[2].parse().map_err(|_| "invalid PPM height")?;
    let count = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
    let data = bytes.get(index..index + count * 3).ok_or("truncated PPM pixels")?;
    let mut pixels = Vec::with_capacity(count * 4);
    for rgb in data.as_chunks::<3>().0 {
        pixels.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    Ok(Image { width, height, pixels })
}

impl Desktop for X11 {
    fn describe(&self) -> serde_json::Value {
        let first_line = |arguments: &[&str]| {
            self.tool(arguments).ok().map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
        };
        serde_json::json!({
            "kind": "x11",
            "display": self.display.to_string_lossy(),
            "server": if self.server.is_some() {
                format!("private Xvfb {}", XVFB_ARGUMENTS.join(" "))
            } else {
                "the caller's display (BARELINE_QA_DISPLAY=inherit)".to_owned()
            },
            "session_bus": self.bus.as_ref().map(|bus| bus.address.clone()).map_err(Clone::clone),
            "caller_display": std::env::var("DISPLAY").ok(),
            "caller_wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
            "xdotool": first_line(&["xdotool", "version"]),
            "imagemagick": first_line(&["import", "-version"]),
            "dialog_host": format!("{:?}", self.dialog_host()),
        })
    }

    fn environment(&self) -> Vec<(&'static str, Option<OsString>)> {
        let mut variables = vec![("DISPLAY", Some(self.display.clone())), ("WAYLAND_DISPLAY", None)];
        if self.server.is_some() {
            // A private display never borrows the caller's bus: a real
            // portal there would show its chooser on the caller's screen.
            let bus = self.bus.as_ref().ok().map(|bus| OsString::from(&bus.address));
            variables.push(("DBUS_SESSION_BUS_ADDRESS", bus));
        }
        variables
    }

    fn answer_next_chooser(&self, target: &Path) -> Result<Option<usize>, Failure> {
        let Ok(bus) = &self.bus else {
            return Ok(None);
        };
        let mut line = target.as_os_str().to_owned();
        line.push("\n");
        std::fs::write(&bus.answers, line.as_encoded_bytes())
            .map_err(|error| Failure::harness(format!("portal answer: {error}")))?;
        Ok(Some(self.chooser_requests().len()))
    }

    fn chooser_requests(&self) -> Vec<serde_json::Value> {
        let Ok(bus) = &self.bus else {
            return Vec::new();
        };
        std::fs::read_to_string(&bus.requests)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn windows(&self, pid: u32) -> Result<Vec<Window>, Failure> {
        self.search(&["xdotool", "search", "--onlyvisible", "--pid", &pid.to_string()])
    }

    fn all_windows(&self) -> Result<Vec<Window>, Failure> {
        self.search(&["xdotool", "search", "--onlyvisible", "--name", ""])
    }

    fn focus(&self, window: &Window) -> Result<(), Failure> {
        // Xvfb runs no window manager, so _NET_ACTIVE_WINDOW (windowactivate)
        // is unavailable; XSetInputFocus directs the XTEST events.
        self.checked(&["xdotool", "windowfocus", "--sync", &window.id])
            .map(|_| ())
    }

    fn key(&self, window: &Window, chord: &str) -> Result<(), Failure> {
        self.focus(window)?;
        self.checked(&["xdotool", "key", &x11_chord(chord)?]).map(|_| ())
    }

    fn text(&self, window: &Window, text: &str) -> Result<(), Failure> {
        self.focus(window)?;
        for (run, ascii) in typing_runs(text) {
            let delay = if ascii { ASCII_DELAY_MS } else { UNICODE_DELAY_MS };
            self.checked(&["xdotool", "type", "--delay", delay, "--", run])?;
        }
        Ok(())
    }

    fn capture(&self, window: &Window) -> Result<Image, Failure> {
        let output = self.tool(&["import", "-window", &window.id, "-depth", "8", "ppm:-"])?;
        if !output.status.success() {
            return Err(Failure::harness(format!(
                "capture failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        parse_ppm(&output.stdout).map_err(|error| Failure::harness(format!("capture: {error}")))
    }

    fn content_top(&self, _window: &Window, _image: &Image) -> i32 {
        0
    }

    fn png(&self, bmp: &Path, png: &Path) -> bool {
        let (Some(bmp), Some(png)) = (bmp.to_str(), png.to_str()) else {
            return false;
        };
        self.tool(&["convert", bmp, png])
            .is_ok_and(|output| output.status.success())
    }

    fn dialog_host(&self) -> Result<String, String> {
        if self.server.is_some() {
            return match &self.bus {
                Ok(bus) if bus_names(&bus.address, "ListNames").contains(PORTAL) => {
                    Ok(format!("the harness's {PORTAL} stand-in on a private session bus"))
                }
                Ok(_) => Err(format!("the portal stand-in no longer owns {PORTAL}")),
                Err(why) => Err(why.clone()),
            };
        }
        // The portal is reached over the session bus the editor is given.
        let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").or_else(|| {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
            let bus = Path::new(&runtime).join("bus");
            bus.exists().then(|| format!("unix:path={}", bus.display()).into())
        }) else {
            return Err("no D-Bus session bus in this session".into());
        };
        if bus_names(&bus, "ListNames").contains(PORTAL) || bus_names(&bus, "ListActivatableNames").contains(PORTAL) {
            Ok(format!("{PORTAL} on the session bus"))
        } else {
            Err(format!("the session bus offers no {PORTAL}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_chords_become_xdotool_key_names() {
        assert_eq!(x11_chord("Primary+Shift+P").unwrap(), "ctrl+shift+p");
        assert_eq!(x11_chord("Alt+Shift+Down").unwrap(), "alt+shift+Down");
        assert_eq!(x11_chord("PageDown").unwrap(), "Next");
        assert_eq!(x11_chord("Primary+Space").unwrap(), "ctrl+space");
        assert_eq!(x11_chord("F6").unwrap(), "F6");
        assert!(x11_chord("Primary+Hyper").is_err());
        assert!(x11_chord("F13").is_err());
    }

    #[test]
    fn typing_runs_separate_ascii_from_characters_outside_the_keymap() {
        assert_eq!(
            typing_runs("A\u{1F389}e\u{301}\u{6587} x"),
            [
                ("A", true),
                ("\u{1F389}", false),
                ("e", true),
                ("\u{301}\u{6587}", false),
                (" x", true)
            ]
        );
        assert!(typing_runs("").is_empty());
    }

    #[test]
    fn a_ppm_capture_becomes_top_down_bgra() {
        let mut ppm = b"P6\n# comment\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[10, 20, 30, 40, 50, 60]);
        let image = parse_ppm(&ppm).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgb(0, 0), (10, 20, 30));
        assert_eq!(image.rgb(1, 0), (40, 50, 60));
        assert!(parse_ppm(b"P6\n2 2\n255\n\x00\x00").is_err());
        assert!(parse_ppm(b"P3\n1 1\n255\n0 0 0").is_err());
    }

    /// Kills the probe window's process when the test ends, also when an
    /// assertion fails first.
    struct Probe(std::process::Child);

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bareline-journey-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    }

    /// Whether this machine can host a private display; the display tests
    /// run whenever it can (they never touch the caller's DISPLAY) and say
    /// why they did nothing otherwise.
    fn private_display_available() -> bool {
        let found = |tool: &str| {
            Command::new("sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .is_ok_and(|output| output.status.success())
        };
        let available = ["Xvfb", "xdotool", "import", "xmessage"].iter().all(|tool| found(tool));
        if !available {
            eprintln!("skipped: needs Xvfb, xdotool, ImageMagick and xmessage (x11-utils)");
        }
        available
    }

    #[test]
    fn the_private_server_chooses_its_own_display_and_never_resets() {
        let arguments = XVFB_ARGUMENTS.join(" ");
        // -displayfd: the server says when it accepts clients on its display.
        assert!(arguments.starts_with("-displayfd 1 "), "{arguments}");
        assert!(arguments.contains("-nolisten tcp"), "{arguments}");
        // -noreset: the last client's disconnect (an editor exit) does not reset the server.
        assert!(arguments.contains("-noreset"), "{arguments}");
        assert!(arguments.contains("-screen 0 1600x1000x24"), "{arguments}");
        assert!(x_connection_refused(
            "event=startup_failed error=os error at winit/src/platform_impl/linux/mod.rs:788: \
             Failed to open connection to X server"
        ));
        assert!(!x_connection_refused("event=startup_failed error=no adapter"));
    }

    /// The display a journey attempt runs on is its own: started on a free
    /// display, it outlives the disconnects of short-lived clients (the
    /// xdotool calls between an editor's exit and the next launch), and it
    /// is gone with the desktop.
    #[test]
    fn a_private_display_serves_back_to_back_clients_and_ends_with_the_desktop() {
        if !private_display_available() {
            return;
        }
        let scratch = scratch("display");
        let first = X11::start(&root(), &scratch).expect("a private Xvfb");
        let second = X11::start(&root(), &scratch).expect("a second private Xvfb beside the first");
        assert_ne!(first.display, second.display, "each desktop has its own display");
        drop(second);
        // Every client disconnects before the next connects: the server is
        // left without clients forty times and must accept each new one.
        for round in 0..40 {
            let output = first.tool(&["xdotool", "getdisplaygeometry"]).unwrap();
            assert!(
                output.status.success(),
                "round {round}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1600 1000");
        }
        let number: u32 = first.display.to_string_lossy().trim_start_matches(':').parse().unwrap();
        assert!(display_in_use(number));
        drop(first);
        let deadline = Instant::now() + Duration::from_secs(5);
        while display_in_use(number) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!display_in_use(number), "the private server outlived its desktop");
        let _ = std::fs::remove_dir_all(scratch);
    }

    /// The portal stand-in answers a real FileChooser call on the private
    /// bus with the staged path, and cancels a request nothing was staged for.
    #[test]
    fn the_portal_stand_in_answers_the_staged_path_on_the_private_bus() {
        if !private_display_available() {
            return;
        }
        let scratch = scratch("portal");
        let desktop = X11::start(&root(), &scratch).expect("a private Xvfb");
        let bus = match &desktop.bus {
            Ok(bus) => bus.address.clone(),
            Err(why) => {
                eprintln!("skipped: no private bus with the portal stand-in: {why}");
                return;
            }
        };
        assert!(desktop.dialog_host().is_ok(), "{:?}", desktop.dialog_host());
        let variables = desktop.environment();
        assert!(variables.contains(&("DBUS_SESSION_BUS_ADDRESS", Some(OsString::from(&bus)))));
        assert!(variables.contains(&("WAYLAND_DISPLAY", None)));
        let target = scratch.join("chosen folder/notes ü.txt");
        assert_eq!(desktop.answer_next_chooser(&target).unwrap(), Some(0));
        // A client as the editor is one: call OpenFile, wait for the Response.
        let client = "import sys\nfrom gi.repository import Gio, GLib\n\
            bus = Gio.DBusConnection.new_for_address_sync(sys.argv[1], \
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)\n\
            loop = GLib.MainLoop()\nanswers = []\n\
            def done(c, s, p, i, n, parameters):\n    answers.append(parameters.unpack())\n    loop.quit()\n\
            bus.signal_subscribe(None, 'org.freedesktop.portal.Request', 'Response', None, None, 0, done)\n\
            options = {'handle_token': GLib.Variant('s', 't1'), 'directory': GLib.Variant('b', False)}\n\
            for _ in range(2):\n    bus.call_sync('org.freedesktop.portal.Desktop', '/org/freedesktop/portal/desktop', \
            'org.freedesktop.portal.FileChooser', 'OpenFile', GLib.Variant('(ssa{sv})', ('', 'Open', options)), \
            None, 0, 5000, None)\n    GLib.timeout_add(5000, loop.quit)\n    loop.run()\n\
            print(answers)";
        let output = output_within(
            Command::new("/usr/bin/python3").args(["-c", client, &bus]),
            "portal client",
            Duration::from_secs(20),
        )
        .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let printed = String::from_utf8_lossy(&output.stdout);
        let uri = "file://".to_owned() + &target.to_string_lossy().replace(' ', "%20").replace('ü', "%C3%BC");
        assert!(printed.contains(&format!("(0, {{'uris': ['{uri}']}})")), "{printed}");
        assert!(
            printed.contains("(1, {'uris': []})"),
            "the second request is cancelled: {printed}"
        );
        let requests = desktop.chooser_requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[0]["method"], "OpenFile");
        assert_eq!(requests[0]["uris"][0], uri.as_str());
        assert_eq!(requests[1]["response"], 1);
        drop(desktop);
        let _ = std::fs::remove_dir_all(scratch);
    }

    /// Runs on a private Xvfb whenever the tools are installed (CI's build job
    /// installs them); the probe window never opens on the caller's DISPLAY.
    #[test]
    fn finds_and_captures_a_window_under_xvfb() {
        if !private_display_available() {
            return;
        }
        let scratch = scratch("window");
        let desktop = desktop(&root(), &scratch).expect("an X11 desktop with xdotool and ImageMagick");
        let display = desktop
            .environment()
            .into_iter()
            .find_map(|(name, value)| (name == "DISPLAY").then_some(value).flatten())
            .expect("the desktop names its display");
        // xmessage ships with x11-utils, like the tools the journeys install.
        // Xt sets no _NET_WM_PID (winit does), so the window is found by title.
        let title = format!("bareline-journey-probe-{}", std::process::id());
        let _probe = Probe(
            Command::new("xmessage")
                .args([
                    "-title",
                    &title,
                    "-geometry",
                    "320x120+10+10",
                    "bareline journey harness probe",
                ])
                .env("DISPLAY", display)
                .spawn()
                .expect("xmessage (x11-utils)"),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let window = loop {
            let windows = desktop.all_windows().unwrap();
            if let Some(window) = windows
                .into_iter()
                .find(|window| window.title.as_deref() == Some(&title))
            {
                break window;
            }
            assert!(Instant::now() < deadline, "no window titled {title} within 10 s");
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(window.width >= 100 && window.height >= 50, "{window:?}");
        let image = desktop.capture(&window).unwrap();
        assert_eq!((image.width, image.height), (window.width, window.height));
        desktop.focus(&window).unwrap();
        let _ = std::fs::remove_dir_all(scratch);
    }
}
