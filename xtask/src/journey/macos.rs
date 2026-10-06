// SPDX-License-Identifier: MPL-2.0
//! The macOS desktop of the journeys, for the logged-in GUI session of a
//! hosted runner. tests/e2e/macos_desktop.py reads the Quartz window list and
//! posts keyboard events to the editor's process (CGEventPostToPid, which
//! needs the Accessibility permission but no keyboard focus); `screencapture
//! -l` captures one window and `sips` converts it to BMP for the pixel checks.
//! Nothing here has run on a Mac yet: the first macos-latest run qualifies it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::Image;
use super::ordinary::{Desktop, Window};
use super::steps::Failure;

pub(super) fn desktop() -> Result<Box<dyn Desktop>, Failure> {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/e2e/macos_desktop.py");
    let desktop = Quartz {
        helper,
        scratch: std::env::temp_dir().join(format!("bareline-journey-capture-{}", std::process::id())),
    };
    std::fs::create_dir_all(&desktop.scratch).map_err(|error| Failure::harness(error.to_string()))?;
    let trusted = desktop.helper(&["trusted"])?;
    if String::from_utf8_lossy(&trusted.stdout).trim() != "true" {
        return Err(Failure::environment(
            "Synthetic input needs the Accessibility permission for the runner process \
             (System Settings > Privacy & Security > Accessibility); AXIsProcessTrusted is false",
        ));
    }
    Ok(Box::new(desktop))
}

struct Quartz {
    helper: PathBuf,
    scratch: PathBuf,
}

impl Quartz {
    fn helper(&self, arguments: &[&str]) -> Result<Output, Failure> {
        let python = std::env::var_os("BARELINE_QA_PYTHON").unwrap_or_else(|| "python3".into());
        let output = Command::new(python)
            .arg(&self.helper)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Failure::environment(format!("python3 is not available: {error}")))?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(Failure::environment(format!(
                "macOS desktop helper {}: {}",
                arguments.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn list(&self, arguments: &[&str]) -> Result<Vec<Window>, Failure> {
        let output = self.helper(arguments)?;
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|error| Failure::harness(format!("window list: {error}")))?;
        Ok(rows
            .iter()
            .map(|row| {
                let number = |key: &str| {
                    row[key]
                        .as_i64()
                        .and_then(|value| i32::try_from(value).ok())
                        .unwrap_or(0)
                };
                Window {
                    id: row["id"].as_str().unwrap_or_default().to_owned(),
                    pid: row["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()),
                    x: number("x"),
                    y: number("y"),
                    width: number("width"),
                    height: number("height"),
                    title: row["title"].as_str().map(str::to_owned),
                }
            })
            .collect())
    }

    fn tool(&self, arguments: &[&str]) -> Result<(), Failure> {
        let output = Command::new(arguments[0])
            .args(&arguments[1..])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Failure::environment(format!("{} is not available: {error}", arguments[0])))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Failure::environment(format!(
                "{} failed: {}",
                arguments[0],
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn pid(window: &Window) -> Result<String, Failure> {
        window
            .pid
            .map(|pid| pid.to_string())
            .ok_or_else(|| Failure::harness("window without an owning process"))
    }
}

/// A 24- or 32-bit uncompressed BMP (as `sips` writes it) as a top-down BGRA image.
fn parse_bmp(bytes: &[u8]) -> Result<Image, String> {
    let word = |at: usize| bytes.get(at..at + 2).map(|raw| u16::from_le_bytes([raw[0], raw[1]]));
    let long = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|raw| i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    };
    if bytes.get(..2) != Some(b"BM") {
        return Err("not a BMP".into());
    }
    let offset = usize::try_from(long(10).ok_or("truncated BMP")?).map_err(|_| "invalid BMP offset")?;
    let width = long(18).ok_or("truncated BMP")?;
    let raw_height = long(22).ok_or("truncated BMP")?;
    let bits = word(28).ok_or("truncated BMP")?;
    let compression = long(30).ok_or("truncated BMP")?;
    // BI_RGB, or BI_BITFIELDS with the usual BGRA masks.
    if !matches!(bits, 24 | 32) || !matches!(compression, 0 | 3) || width <= 0 || raw_height == 0 {
        return Err(format!("unsupported BMP: {bits} bits, compression {compression}"));
    }
    let height = raw_height.abs();
    let bytes_per_pixel = usize::from(bits / 8);
    let columns = usize::try_from(width).map_err(|_| "invalid BMP width")?;
    let stride = (columns * bytes_per_pixel).div_ceil(4) * 4;
    let mut pixels = Vec::with_capacity(columns * usize::try_from(height).unwrap_or(0) * 4);
    for row in 0..height {
        let source = if raw_height > 0 { height - 1 - row } else { row };
        let start = offset + usize::try_from(source).unwrap_or(0) * stride;
        let line = bytes
            .get(start..start + columns * bytes_per_pixel)
            .ok_or("truncated BMP pixels")?;
        for pixel in line.chunks_exact(bytes_per_pixel) {
            pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
    }
    Ok(Image { width, height, pixels })
}

impl Desktop for Quartz {
    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "quartz",
            "helper": self.helper,
            "input": "CGEventPostToPid through tests/e2e/macos_desktop.py",
            "capture": "screencapture -l, sips",
        })
    }

    fn windows(&self, pid: u32) -> Result<Vec<Window>, Failure> {
        self.list(&["windows", &pid.to_string()])
    }

    fn all_windows(&self) -> Result<Vec<Window>, Failure> {
        self.list(&["windows"])
    }

    fn focus(&self, window: &Window) -> Result<(), Failure> {
        // Events are posted to the process, so focus only keeps the window in
        // front for the captures; a refusal (no Automation permission) is fine.
        let script = format!(
            "tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true",
            Self::pid(window)?
        );
        let _ = Command::new("osascript")
            .args(["-e", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }

    fn key(&self, window: &Window, chord: &str) -> Result<(), Failure> {
        self.helper(&["key", &Self::pid(window)?, chord]).map(|_| ())
    }

    fn text(&self, window: &Window, text: &str) -> Result<(), Failure> {
        self.helper(&["text", &Self::pid(window)?, text]).map(|_| ())
    }

    fn capture(&self, window: &Window) -> Result<Image, Failure> {
        let png = self.scratch.join(format!("window-{}.png", window.id));
        let bmp = png.with_extension("bmp");
        let (Some(png_path), Some(bmp_path)) = (png.to_str(), bmp.to_str()) else {
            return Err(Failure::harness("capture path is not UTF-8"));
        };
        self.tool(&["screencapture", "-x", "-o", "-l", &window.id, "-t", "png", png_path])?;
        self.tool(&["sips", "-s", "format", "bmp", png_path, "--out", bmp_path])?;
        let bytes = std::fs::read(&bmp).map_err(|error| Failure::harness(error.to_string()))?;
        parse_bmp(&bytes).map_err(|error| Failure::harness(format!("capture: {error}")))
    }

    fn content_top(&self, window: &Window, image: &Image) -> i32 {
        // The captured frame includes the standard 28-point title bar.
        let scale = if window.width > 0 {
            (image.width / window.width).max(1)
        } else {
            1
        };
        28 * scale
    }

    fn png(&self, bmp: &Path, png: &Path) -> bool {
        let (Some(bmp), Some(png)) = (bmp.to_str(), png.to_str()) else {
            return false;
        };
        self.tool(&["sips", "-s", "format", "png", bmp, "--out", png]).is_ok()
    }

    fn dialog_host(&self) -> Result<String, String> {
        Ok("AppKit panels in the logged-in GUI session".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_up_and_top_down_bmps_read_the_same_pixels() {
        let image = Image {
            width: 3,
            height: 2,
            pixels: (0u8..24).collect(),
        };
        let path = std::env::temp_dir().join(format!("bareline-journey-bmp-{}.bmp", std::process::id()));
        super::super::save_bmp(&image, &path).unwrap();
        let read = parse_bmp(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((read.width, read.height), (3, 2));
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(read.rgb(x, y), image.rgb(x, y));
            }
        }
        assert!(parse_bmp(b"BMtruncated").is_err());
    }
}
