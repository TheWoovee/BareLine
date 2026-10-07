// SPDX-License-Identifier: MPL-2.0
//! The X11 desktop of the Linux journeys: xdotool finds windows by their
//! _NET_WM_PID, focuses them and types through XTEST; ImageMagick's `import`
//! captures a window as PPM. The journeys run under Xvfb (no window manager)
//! with WAYLAND_DISPLAY unset, so winit uses X11 where these tools see it.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use super::Image;
use super::ordinary::{Desktop, Window, output_within};
use super::steps::Failure;

const TOOL_DEADLINE: Duration = Duration::from_secs(20);
/// Pacing between typed characters. A character outside the keymap makes
/// xdotool rebind a spare keycode; winit must see that mapping change before
/// the key event, which takes far longer than an ASCII keystroke.
const ASCII_DELAY_MS: &str = "12";
const UNICODE_DELAY_MS: &str = "90";

pub(super) fn desktop() -> Result<Box<dyn Desktop>, Failure> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(Failure::environment(
            "No X11 DISPLAY: run the journeys under xvfb-run (xdotool cannot see Wayland windows)",
        ));
    }
    let desktop = X11;
    let xdotool = desktop.tool(&["xdotool", "version"])?;
    if !xdotool.status.success() {
        return Err(Failure::environment("xdotool is not usable in this session"));
    }
    desktop.tool(&["import", "-version"])?;
    Ok(Box::new(desktop))
}

struct X11;

impl X11 {
    /// Run a tool with a deadline; a missing tool is an environment failure.
    fn tool(&self, arguments: &[&str]) -> Result<Output, Failure> {
        output_within(
            Command::new(arguments[0]).args(&arguments[1..]),
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
    for rgb in data.chunks_exact(3) {
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
            "display": std::env::var("DISPLAY").ok(),
            "wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
            "xdotool": first_line(&["xdotool", "version"]),
            "imagemagick": first_line(&["import", "-version"]),
            "dialog_host": format!("{:?}", self.dialog_host()),
        })
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
        // The portal is reached over the session bus the editor is given.
        let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").or_else(|| {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
            let bus = Path::new(&runtime).join("bus");
            bus.exists().then(|| format!("unix:path={}", bus.display()).into())
        }) else {
            return Err("no D-Bus session bus in this session".into());
        };
        // dbus-send waits up to the D-Bus default (about 25 s) for a reply.
        let names = |method: &str| {
            output_within(
                Command::new("dbus-send").env("DBUS_SESSION_BUS_ADDRESS", &bus).args([
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
        };
        let portal = "org.freedesktop.portal.Desktop";
        if names("ListNames").contains(portal) || names("ListActivatableNames").contains(portal) {
            Ok(format!("{portal} on the session bus"))
        } else {
            Err(format!("the session bus offers no {portal}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    /// Needs a disposable X display (xvfb-run). It stays ignored rather than
    /// gated on DISPLAY alone: a desktop session (WSLg included) sets DISPLAY
    /// too, and the probe window would open on the user's screen. CI runs it
    /// with --ignored under xvfb-run; without DISPLAY it returns at once.
    #[test]
    #[ignore = "needs an X11 display with xdotool, ImageMagick and xmessage: \
                xvfb-run -a cargo test -p xtask -- --ignored finds_and_captures"]
    fn finds_and_captures_a_window_under_xvfb() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("no DISPLAY: run under xvfb-run to exercise the X11 desktop");
            return;
        }
        let desktop = desktop().expect("an X11 desktop with xdotool and ImageMagick");
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
    }
}
