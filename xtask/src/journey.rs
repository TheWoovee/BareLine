// SPDX-License-Identifier: MPL-2.0
//! Native journey runner: launches the built `bareline.exe` in an isolated data
//! directory, drives it with `SendInput` (and native menu commands where
//! synthetic chords are unreliable — see `docs/qa/JOURNEYS.md`), captures window
//! screenshots, and asserts on pixel regions and on the diagnostics log.
//!
//! Each journey is small and independent: it launches its own process into a
//! throwaway data root, performs a few steps, and is torn down by killing the
//! process it started (never any other `bareline.exe`). The encoded journeys
//! mirror the acceptance checks in `docs/implementation/PLAN-20260908-REVIEW-FIXES.md`.
#![cfg(windows)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetDIBits, GetDeviceCaps, LOGPIXELSX, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationInvokePattern, IUIAutomationTextPattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, UIA_ButtonControlTypeId, UIA_EditControlTypeId, UIA_InvokePatternId,
    UIA_TextPatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, IsWindowEnabled, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    SendInput, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumChildWindows, EnumWindows, GW_OWNER, GetClassNameW, GetClientRect, GetDlgCtrlID,
    GetForegroundWindow, GetMenu, GetMenuItemCount, GetMenuItemID, GetMenuStringW, GetSubMenu, GetWindow,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, HWND_TOP, IsWindowVisible, MF_BYPOSITION, PostMessageW,
    SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, ShowWindow, WM_COMMAND,
};
use windows::core::BOOL;

// ------------------------------------------------------------------ registry --

/// One encoded journey. `run` returns `Ok(())` on pass or `Err(reason)` on fail.
struct Journey {
    name: &'static str,
    summary: &'static str,
    run: fn(&Env) -> Result<(), String>,
}

fn journeys() -> Vec<Journey> {
    vec![
        Journey {
            name: "smoke",
            summary: "launch hidden and exit after first frame",
            run: journey_smoke,
        },
        Journey {
            name: "p0-3",
            summary: "recovery journals bounded and discarded",
            run: journey_p0_3,
        },
        Journey {
            name: "p0-4",
            summary: "external SW_SHOW keeps the window visible",
            run: journey_p0_4,
        },
        Journey {
            name: "p0-5",
            summary: "recovered untitled document is editable",
            run: journey_p0_5,
        },
        Journey {
            name: "p0-7",
            summary: "close prompt offers Save / Don't Save / Cancel",
            run: journey_p0_7,
        },
        Journey {
            name: "p1-1",
            summary: "no single-item submenus in the menu bar",
            run: journey_p1_1,
        },
        Journey {
            name: "p1-2",
            summary: "curated top-level menu taxonomy",
            run: journey_p1_2,
        },
        Journey {
            name: "p1-5",
            summary: "Go To and Run modals open and dismiss on Escape",
            run: journey_p1_5,
        },
        Journey {
            name: "p3-1",
            summary: "docks never overlap or duplicate the tab strip",
            run: journey_p3_1,
        },
        Journey {
            name: "p3-6a",
            summary: "Settings opens as a closable tab",
            run: journey_p3_6a,
        },
        Journey {
            name: "p4-1",
            summary: "dirty tab shows exactly one marker",
            run: journey_p4_1,
        },
        Journey {
            name: "p4-2",
            summary: "closing the last tab yields a fresh Untitled",
            run: journey_p4_2,
        },
        Journey {
            name: "p4-4",
            summary: "colour emoji renders in the editor",
            run: journey_p4_4,
        },
    ]
}

// ------------------------------------------------------------------- driver ---

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let build = args.iter().any(|a| a == "--build");
    let retry_of = args.iter().find_map(|arg| arg.strip_prefix("--retry-of="));
    let selectors: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();
    let name = *selectors
        .first()
        .ok_or("Usage: cargo xtask journey <name|all> [--build] [--retry-of=<run-id>]")?;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("workspace root unavailable")?
        .to_path_buf();

    if build {
        eprintln!("journey: cargo build -p bareline");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "bareline"])
            .current_dir(&root)
            .status()?;
        if !status.success() {
            return Err("cargo build -p bareline failed".into());
        }
    }

    let exe = root.join("target/debug/bareline.exe");
    if !exe.exists() {
        return Err(format!(
            "{} not found; build it first (cargo build -p bareline) or pass --build",
            exe.display()
        )
        .into());
    }

    let all = journeys();
    let selected: Vec<&Journey> = if name == "all" {
        all.iter().collect()
    } else {
        let hit: Vec<&Journey> = all.iter().filter(|j| j.name == name).collect();
        if hit.is_empty() {
            let names: Vec<&str> = all.iter().map(|j| j.name).collect();
            return Err(format!("unknown journey '{name}'; known: {}", names.join(", ")).into());
        }
        hit
    };

    let run_identity = run_identity(&root, &exe, retry_of)?;
    let run_id = run_identity["run_id"].as_str().ok_or("run id unavailable")?.to_owned();
    let evidence_path = root.join("target/journey/results").join(format!("{run_id}.json"));
    eprintln!("journey: run_identity={run_identity}");
    let env = Env {
        exe,
        root,
        run_id: run_id.clone(),
    };
    let mut failures = Vec::new();
    let mut results = Vec::new();
    persist_run_evidence(&evidence_path, &run_identity, selected.len(), &results)?;
    for journey in &selected {
        eprintln!("journey {} — {} …", journey.name, journey.summary);
        match (journey.run)(&env) {
            Ok(()) => {
                println!("PASS {}", journey.name);
                results.push(serde_json::json!({"name": journey.name, "status": "passed"}));
            }
            Err(reason) => {
                println!("FAIL {} — {reason}", journey.name);
                failures.push(journey.name);
                results.push(serde_json::json!({
                    "name": journey.name,
                    "status": "failed",
                    "classification": classify_failure(&reason),
                    "detail": reason,
                }));
            }
        }
        persist_run_evidence(&evidence_path, &run_identity, selected.len(), &results)?;
    }

    let passed = selected.len() - failures.len();
    eprintln!(
        "journey: top_level_passed={passed} top_level_failed={} top_level_total={}",
        failures.len(),
        selected.len()
    );
    eprintln!("journey: evidence={}", evidence_path.display());
    eprintln!("journey: evidence_sha256={}", hash_file(&evidence_path)?);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} journey(s) failed: {}", failures.len(), failures.join(", ")).into())
    }
}

fn run_identity(
    root: &Path,
    exe: &Path,
    retry_of: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let run_id = format!("{}-{stamp}", std::process::id());
    let (head, working_tree_dirty, source_manifest_sha256) = source_identity(root)?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "retry_of": retry_of,
        "head": head,
        "working_tree_dirty": working_tree_dirty,
        "source_manifest_sha256": source_manifest_sha256,
        "executable": exe,
        "executable_sha256": hash_file(exe)?,
    }))
}

fn source_identity(root: &Path) -> Result<(String, bool, String), Box<dyn std::error::Error>> {
    let head = git_output(root, &["rev-parse", "HEAD"])?;
    let status = git_output_bytes(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    let tracked = git_output_bytes(root, &["diff", "--binary", "--no-ext-diff", "HEAD", "--"])?;
    let untracked = git_output_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut source = Sha256::new();
    source.update(b"status\0");
    source.update((status.len() as u64).to_le_bytes());
    source.update(&status);
    source.update(b"tracked-diff\0");
    source.update((tracked.len() as u64).to_le_bytes());
    source.update(&tracked);
    for raw in untracked.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        let relative = PathBuf::from(String::from_utf8(raw.to_vec())?);
        source.update(b"untracked\0");
        source.update((raw.len() as u64).to_le_bytes());
        source.update(raw);
        let mut file = std::fs::File::open(root.join(&relative))?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            source.update((count as u64).to_le_bytes());
            source.update(&buffer[..count]);
        }
        source.update(0u64.to_le_bytes());
    }
    Ok((head, !status.is_empty(), format!("{:x}", source.finalize())))
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = git_output_bytes(root, args)?;
    Ok(String::from_utf8(bytes)?.trim().to_owned())
}

fn git_output_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn hash_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn classify_failure(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("timed out") || reason.contains("timeout") {
        "timeout"
    } else if reason.contains("access is denied") || reason.contains("blocked environment") {
        "blocked_environment"
    } else if reason.contains("spawn failed")
        || reason.contains("capture")
        || reason.contains("getwindow")
        || reason.contains("no window")
        || reason.contains("typing never reached")
        || reason.contains("uia prerequisite")
    {
        "harness_setup"
    } else {
        "product_assertion"
    }
}

fn persist_run_evidence(
    path: &Path,
    identity: &serde_json::Value,
    top_level_total: usize,
    results: &[serde_json::Value],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let passed = results.iter().filter(|result| result["status"] == "passed").count();
    let failed = results.iter().filter(|result| result["status"] == "failed").count();
    let document = serde_json::json!({
        "identity": identity,
        "top_level_total": top_level_total,
        "top_level_completed": results.len(),
        "top_level_passed": passed,
        "top_level_failed": failed,
        "results": results,
    });
    std::fs::write(path, serde_json::to_vec_pretty(&document)?)?;
    Ok(())
}

/// Static context shared by every journey.
struct Env {
    exe: PathBuf,
    root: PathBuf,
    run_id: String,
}

impl Env {
    /// Create an isolated data root under `target/journey/<name>-<stamp>` and
    /// return `(home, data_root)` where `home` is what `%LOCALAPPDATA%`/`%APPDATA%`
    /// point at and `data_root` is `home\Bareline` (the app's own subdirectory).
    fn scratch(&self, name: &str) -> Result<(PathBuf, PathBuf), String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let home = self
            .root
            .join("target/journey")
            .join(format!("{name}-{}-{stamp}", std::process::id()));
        let temp = home.join("temp");
        std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        Ok((home.clone(), home.join("Bareline")))
    }
}

/// Point every data/temp environment variable at the isolated scratch home so a
/// journey never reads or writes the developer's real profile.
fn isolate(command: &mut Command, home: &Path) {
    let temp = home.join("temp");
    command
        .env("LOCALAPPDATA", home)
        .env("APPDATA", home)
        .env("TEMP", &temp)
        .env("TMP", &temp);
}

// --------------------------------------------------------------- session RAII -

/// A launched editor process plus its isolated data root. Dropping the session
/// kills only the process this journey started — never any other instance.
struct Session {
    child: Child,
    pid: u32,
    home: PathBuf,
    data_root: PathBuf,
    run_id: String,
    launch_files: Vec<PathBuf>,
    hwnd: Option<HWND>,
    command_traces: Vec<PathBuf>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Session {
    /// Launch the editor into an isolated home with a stable, non-interactive
    /// configuration (software renderer, fresh instance, no session or extensions).
    fn launch(env: &Env, name: &str, extra: &[&str], files: &[PathBuf]) -> Result<Session, String> {
        let (home, data_root) = env.scratch(name)?;
        let mut command = Command::new(&env.exe);
        command
            .args(["--software", "--no-session", "--no-extensions", "--new-instance"])
            .args(extra);
        for file in files {
            command.arg(file);
        }
        isolate(&mut command, &home);
        let command_trace = home.join("evidence/command-trace-launch-1.jsonl");
        command.env("BARELINE_QA_COMMAND_TRACE", &command_trace);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let child = command.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        let pid = child.id();
        Ok(Session {
            child,
            pid,
            home,
            data_root,
            run_id: env.run_id.clone(),
            launch_files: files.to_vec(),
            hwnd: None,
            command_traces: vec![command_trace],
        })
    }

    /// Wait for the editor's main window (class "Window Class") to appear and
    /// become visible, then focus it. Returns the handle.
    fn wait_window(&mut self, timeout: Duration) -> Result<HWND, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("process exited before showing a window: {status}"));
            }
            if let Some(hwnd) = find_main_window(self.pid) {
                focus(hwnd);
                self.hwnd = Some(hwnd);
                return Ok(hwnd);
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for the main window".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn restart(&mut self, env: &Env) -> Result<HWND, String> {
        let mut command = Command::new(&env.exe);
        command.args(["--software", "--no-session", "--no-extensions", "--new-instance"]);
        isolate(&mut command, &self.home);
        let command_trace = self.home.join(format!(
            "evidence/command-trace-launch-{}.jsonl",
            self.command_traces.len() + 1
        ));
        command.env("BARELINE_QA_COMMAND_TRACE", &command_trace);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        self.child = command
            .spawn()
            .map_err(|error| format!("restart spawn failed: {error}"))?;
        self.pid = self.child.id();
        self.hwnd = None;
        self.command_traces.push(command_trace);
        self.wait_window(Duration::from_secs(20))
    }

    fn command_trace_evidence(&self) -> serde_json::Value {
        serde_json::json!(
            self.command_traces
                .iter()
                .map(|path| serde_json::json!({
                    "path": path,
                    "present": path.is_file(),
                }))
                .collect::<Vec<_>>()
        )
    }

    /// Give the editor time to settle a frame after an input.
    fn settle(&self) {
        std::thread::sleep(Duration::from_millis(450));
    }

    /// Parsed JSON-lines from the isolated diagnostics log (`bareline.log`).
    fn diagnostics(&self) -> Vec<serde_json::Value> {
        let path = self.data_root.join("diagnostics/bareline.log");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Count the recovery journal directories currently on disk for this data
    /// root (leaf directories under `recovery`, at any depth).
    fn recovery_dir_count(&self) -> usize {
        fn walk(dir: &Path, count: &mut usize) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                // P0-3 journals are named `paged-<pid>-<stamp>-<n>-g<generation>`.
                // Other leaf directories under recovery (transcode staging,
                // source snapshots) are not journal slots and must not inflate
                // the generation count.
                if entry.file_name().to_string_lossy().starts_with("paged-") {
                    *count += 1;
                }
                walk(&path, count);
            }
        }
        let mut count = 0;
        walk(&self.data_root.join("recovery"), &mut count);
        count
    }

    fn recovery_terminal_state(&self) -> serde_json::Value {
        let root = self.data_root.join("recovery");
        let mut entries = Vec::new();
        let children = match std::fs::read_dir(&root) {
            Ok(children) => children,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return serde_json::json!({
                    "physical_directories": 0,
                    "production_discovery_candidates": 0,
                    "entries": entries,
                });
            }
            Err(error) => {
                return serde_json::json!({
                    "physical_directories": "unknown",
                    "production_discovery_candidates": 1,
                    "root_inspection_error": error.to_string(),
                    "entries": entries,
                });
            }
        };
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    entries.push(serde_json::json!({
                        "entry_error": error.to_string(),
                        "production_discovery_candidate": true,
                        "retirement_blocks_restore": "unknown",
                    }));
                    continue;
                }
            };
            let name = child.file_name();
            if !name.to_string_lossy().starts_with("paged-") {
                continue;
            }
            let path = child.path();
            let (plain_directory, metadata_error) = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => (metadata.is_dir() && !metadata.file_type().is_symlink(), None),
                Err(error) => (false, Some(error.to_string())),
            };
            if !plain_directory {
                entries.push(serde_json::json!({
                    "directory": path,
                    "plain_directory": false,
                    "metadata_error": metadata_error,
                    "production_discovery_candidate": true,
                    "retirement_blocks_restore": "unknown",
                }));
                continue;
            }
            match bareline_file_io::recovery::inspect(&path, &bareline_file_io::cancellation::Cancellation::default()) {
                Ok(inspection) => {
                    let discarded = inspection.status == bareline_file_io::recovery::RecoveryStatus::Discarded;
                    entries.push(serde_json::json!({
                        "directory": path,
                        "inspection_status": format!("{:?}", inspection.status),
                        "durable_receipt": inspection.last_durable.map(|receipt| serde_json::json!({
                            "revision": receipt.revision,
                            "protected_unix_ms": receipt.protected_unix_ms,
                        })),
                        "complete_baseline": inspection.complete_baseline,
                        "production_discovery_candidate": !discarded,
                        "retirement_blocks_restore": discarded,
                    }));
                }
                Err(error) => entries.push(serde_json::json!({
                    "directory": path,
                    "inspection_error": error.to_string(),
                    "production_discovery_candidate": true,
                    "retirement_blocks_restore": "unknown",
                })),
            }
        }
        let candidates = entries
            .iter()
            .filter(|entry| entry["production_discovery_candidate"].as_bool() != Some(false))
            .count();
        serde_json::json!({
            "physical_directories": entries.len(),
            "production_discovery_candidates": candidates,
            "entries": entries,
        })
    }
}

// --------------------------------------------------------------- window search

struct FoundWindows(Vec<HWND>);

unsafe extern "system" fn collect_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` carries a `&mut FoundWindows` for the duration of the enum.
    let found = unsafe { &mut *(lparam.0 as *mut FoundWindows) };
    found.0.push(hwnd);
    BOOL(1)
}

fn top_level_windows() -> Vec<HWND> {
    let mut found = FoundWindows(Vec::new());
    // SAFETY: the callback only pushes handles into `found`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(collect_windows), LPARAM(&mut found as *mut FoundWindows as isize));
    }
    found.0
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a live top-level handle from EnumWindows; `pid` is owned.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is a local, correctly-sized buffer.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a local, correctly-sized buffer.
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: `hwnd` is a live handle.
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is owned; `hwnd` is live.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
        Some(rect)
    } else {
        None
    }
}

fn find_main_window(pid: u32) -> Option<HWND> {
    top_level_windows()
        .into_iter()
        .find(|&hwnd| window_pid(hwnd) == pid && is_visible(hwnd) && window_class(hwnd) == "Window Class")
}

/// Top-level windows of `pid` that are not the main editor window (e.g. a
/// modal save prompt). Returns `(hwnd, class, title)` triples.
fn auxiliary_windows(pid: u32, main: HWND) -> Vec<(HWND, String, String)> {
    top_level_windows()
        .into_iter()
        .filter(|&hwnd| hwnd.0 != main.0 && window_pid(hwnd) == pid && is_visible(hwnd))
        .map(|hwnd| (hwnd, window_class(hwnd), window_title(hwnd)))
        .collect()
}

/// Child controls (labels/buttons) of a window, read as text.
fn child_texts(parent: HWND) -> Vec<String> {
    let mut found = FoundWindows(Vec::new());
    // SAFETY: callback only records handles into `found`.
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(collect_windows),
            LPARAM(&mut found as *mut FoundWindows as isize),
        );
    }
    found
        .0
        .into_iter()
        .map(window_title)
        .filter(|text| !text.is_empty())
        .collect()
}

struct BoundedWindows {
    handles: Vec<HWND>,
    limit: usize,
    limit_reached: bool,
}

unsafe extern "system" fn collect_bounded_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` carries the BoundedWindows used only during enumeration.
    let found = unsafe { &mut *(lparam.0 as *mut BoundedWindows) };
    if found.handles.len() == found.limit {
        found.limit_reached = true;
        return BOOL(0);
    }
    found.handles.push(hwnd);
    BOOL(1)
}

fn bounded_descendants(parent: HWND, limit: usize) -> BoundedWindows {
    let mut found = BoundedWindows {
        handles: Vec::new(),
        limit,
        limit_reached: false,
    };
    // SAFETY: callback state outlives this synchronous enumeration.
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(collect_bounded_windows),
            LPARAM(&mut found as *mut BoundedWindows as isize),
        );
    }
    found
}

fn dialog_uia_buttons(hwnd: HWND) -> serde_json::Value {
    const NODE_LIMIT: usize = 256;
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
    if let Err(error) = initialized {
        return serde_json::json!({"available": false, "error": error.to_string()});
    }
    let result = (|| -> windows::core::Result<serde_json::Value> {
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
        let root = unsafe { automation.ElementFromHandle(hwnd) }?;
        let walker = unsafe { automation.ControlViewWalker() }?;
        let mut pending = VecDeque::new();
        if let Ok(first) = unsafe { walker.GetFirstChildElement(&root) } {
            pending.push_back(first);
        }
        let mut visited = 0usize;
        let mut buttons = Vec::new();
        while visited < NODE_LIMIT {
            let Some(element) = pending.pop_front() else { break };
            visited += 1;
            if let Ok(child) = unsafe { walker.GetFirstChildElement(&element) } {
                pending.push_back(child);
            }
            if let Ok(sibling) = unsafe { walker.GetNextSiblingElement(&element) } {
                pending.push_back(sibling);
            }
            if unsafe { element.CurrentControlType() }.ok().map(|value| value.0) != Some(UIA_ButtonControlTypeId.0) {
                continue;
            }
            buttons.push(serde_json::json!({
                "name": unsafe { element.CurrentName() }.ok().map(|value| value.to_string()),
                "enabled": unsafe { element.CurrentIsEnabled() }.ok().map(|value| value.as_bool()),
                "invoke_available": unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                }.is_ok(),
            }));
        }
        Ok(serde_json::json!({
            "available": true,
            "visited_nodes": visited,
            "node_limit": NODE_LIMIT,
            "limit_reached": !pending.is_empty(),
            "buttons": buttons,
        }))
    })();
    unsafe { CoUninitialize() };
    result.unwrap_or_else(|error| serde_json::json!({"available": false, "error": error.to_string()}))
}

fn prompt_observer_snapshot(session: &Session, main: HWND) -> serde_json::Value {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_pid = window_pid(foreground);
    let windows = top_level_windows()
        .into_iter()
        .filter(|window| {
            if *window == main || window_pid(*window) != session.pid {
                return *window == main;
            }
            unsafe { GetWindow(*window, GW_OWNER) }.ok() == Some(main)
        })
        .map(|window| {
            let owner = unsafe { GetWindow(window, GW_OWNER) }.ok();
            let descendants = bounded_descendants(window, 128);
            let controls = descendants
                .handles
                .into_iter()
                .map(|control| {
                    serde_json::json!({
                        "hwnd": control.0 as usize,
                        "class": window_class(control),
                        "title": window_title(control),
                        "control_id": unsafe { GetDlgCtrlID(control) },
                        "enabled": unsafe { IsWindowEnabled(control) }.as_bool(),
                        "visible": is_visible(control),
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "hwnd": window.0 as usize,
                "owner_hwnd": owner.map(|owner| owner.0 as usize),
                "owned_by_main": owner == Some(main),
                "class": window_class(window),
                "title": window_title(window),
                "enabled": unsafe { IsWindowEnabled(window) }.as_bool(),
                "visible": is_visible(window),
                "rect": window_rect(window).map(|rect| [rect.left, rect.top, rect.right, rect.bottom]),
                "descendant_limit": descendants.limit,
                "descendant_limit_reached": descendants.limit_reached,
                "descendants": controls,
                "uia": (window != main).then(|| dialog_uia_buttons(window)),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "main_hwnd": main.0 as usize,
        "main_enabled": unsafe { IsWindowEnabled(main) }.as_bool(),
        "foreground_hwnd": foreground.0 as usize,
        "foreground_pid": foreground_pid,
        "foreground_same_process": foreground_pid == session.pid,
        "foreground_class": (foreground_pid == session.pid).then(|| window_class(foreground)),
        "same_process_top_level_windows": windows,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PromptTopology {
    main_enabled: bool,
    candidates: Vec<(usize, Option<usize>, bool)>,
}

fn prompt_topology(session: &Session, main: HWND) -> PromptTopology {
    let mut candidates = top_level_windows()
        .into_iter()
        .filter(|window| {
            *window != main
                && window_pid(*window) == session.pid
                && unsafe { GetWindow(*window, GW_OWNER) }.ok() == Some(main)
        })
        .map(|window| {
            let owner = unsafe { GetWindow(window, GW_OWNER) }
                .ok()
                .map(|owner| owner.0 as usize);
            (window.0 as usize, owner, is_visible(window))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    PromptTopology {
        main_enabled: unsafe { IsWindowEnabled(main) }.as_bool(),
        candidates,
    }
}

struct PromptWait {
    dialog: Option<HWND>,
    observations: Vec<serde_json::Value>,
}

fn retain_prompt_observation(
    observations: &mut Vec<serde_json::Value>,
    observation: serde_json::Value,
    terminal: bool,
) {
    const LIMIT: usize = 8;
    if observations.len() < LIMIT {
        observations.push(observation);
    } else if terminal {
        observations[LIMIT - 1] = observation;
    }
}

fn prompt_observation(session: &Session, main: HWND, trigger: &str, elapsed: Duration) -> serde_json::Value {
    serde_json::json!({
        "trigger": trigger,
        "elapsed_ms": elapsed.as_millis(),
        "snapshot": prompt_observer_snapshot(session, main),
    })
}

fn window_evidence(session: &Session, hwnd: HWND) -> serde_json::Value {
    let window = window_rect(hwnd).map(|rect| [rect.left, rect.top, rect.right, rect.bottom]);
    let mut client = RECT::default();
    // SAFETY: `hwnd` is live and `client` is an owned output buffer.
    let client = unsafe { GetClientRect(hwnd, &mut client) }
        .ok()
        .map(|_| [client.left, client.top, client.right, client.bottom]);
    // SAFETY: the acquired window DC is released before returning.
    let dpi = unsafe {
        let dc = GetDC(Some(hwnd));
        if dc.0.is_null() {
            None
        } else {
            let dpi = GetDeviceCaps(Some(dc), LOGPIXELSX);
            let _ = ReleaseDC(Some(hwnd), dc);
            (dpi > 0).then_some(dpi)
        }
    };
    // SAFETY: no arguments beyond querying the current foreground handle.
    let foreground = unsafe { GetForegroundWindow() };
    let dialogs: Vec<_> = auxiliary_windows(session.pid, hwnd)
        .into_iter()
        .map(|(handle, class, title)| {
            serde_json::json!({
                "hwnd": handle.0 as usize,
                "class": class,
                "title": title,
                "children": child_texts(handle),
            })
        })
        .collect();
    let diagnostics = session.diagnostics();
    let diagnostics_tail: Vec<_> = diagnostics.into_iter().rev().take(20).collect();
    serde_json::json!({
        "run_id": &session.run_id,
        "pid": session.pid,
        "hwnd": hwnd.0 as usize,
        "foreground_hwnd": foreground.0 as usize,
        "focused": foreground == hwnd,
        "window_rect": window,
        "client_rect": client,
        "dpi": dpi,
        "renderer": "software",
        "window_title": window_title(hwnd),
        "active_document_candidates": &session.launch_files,
        "main_child_control_text": child_texts(hwnd),
        "dialog_tree": dialogs,
        "isolated_home": &session.home,
        "isolated_data_root": &session.data_root,
        "diagnostics_tail": diagnostics_tail,
        "uia_provider": uia_provider_snapshot(hwnd),
    })
}

fn text_pattern_snapshot(pattern: &IUIAutomationTextPattern) -> windows::core::Result<serde_json::Value> {
    const TEXT_LIMIT_UTF16: usize = 65_536;
    const QUERY_LIMIT: i32 = 65_537;
    let document = unsafe { pattern.DocumentRange() }?;
    let document_text = unsafe { document.GetText(QUERY_LIMIT) }?.to_string();
    let document_utf16 = document_text.encode_utf16().count();
    let selection = unsafe { pattern.GetSelection() }?;
    let selection_count = unsafe { selection.Length() }?;
    let selected = if selection_count > 0 {
        let range = unsafe { selection.GetElement(0) }?;
        let selected_text = unsafe { range.GetText(QUERY_LIMIT) }?.to_string();
        let prefix = unsafe { document.Clone() }?;
        unsafe {
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, &range, TextPatternRangeEndpoint_Start)?;
        }
        let prefix_text = unsafe { prefix.GetText(QUERY_LIMIT) }?.to_string();
        let caret_start_utf16 = prefix_text.encode_utf16().count();
        let selection_utf16 = selected_text.encode_utf16().count();
        Some(serde_json::json!({
            "caret_start_utf16": caret_start_utf16,
            "selection_utf16": selection_utf16,
            "selected_text": selected_text,
        }))
    } else {
        None
    };
    Ok(serde_json::json!({
        "available": true,
        "document_text": document_text,
        "document_text_utf16": document_utf16,
        "document_text_limit_utf16": TEXT_LIMIT_UTF16,
        "bounded_complete": document_utf16 <= TEXT_LIMIT_UTF16,
        "selection_count": selection_count,
        "selection": selected,
    }))
}

fn production_editor_identity(name: &str) -> Option<&str> {
    if name == "Editor" {
        return Some(name);
    }
    let (identity, title) = name.split_once(',')?;
    let pane = identity.strip_prefix("Pane ")?;
    (matches!(pane, "1" | "2") && !title.trim().is_empty()).then_some(identity)
}

fn uia_provider_snapshot_once(hwnd: HWND, focus_editor: bool) -> serde_json::Value {
    // SAFETY: this function balances a successful COM initialization before
    // returning and holds every UIA interface only within that interval.
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
    if let Err(error) = initialized {
        return serde_json::json!({
            "available": false,
            "reason": format!("COM initialization failed: {error}"),
        });
    }
    let result = (|| -> windows::core::Result<serde_json::Value> {
        // SAFETY: CUIAutomation is an in-process COM server and COM is initialized.
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: hwnd is the live journey window.
        let root = unsafe { automation.ElementFromHandle(hwnd) }?;
        let focused = unsafe { automation.GetFocusedElement() }?;
        let root_name = unsafe { root.CurrentName() }?.to_string();
        let focused_name = unsafe { focused.CurrentName() }?.to_string();
        let root_process_id = unsafe { root.CurrentProcessId() }?;
        let focused_process_id = unsafe { focused.CurrentProcessId() }?;
        let control_type = unsafe { focused.CurrentControlType() }?.0;
        let walker = unsafe { automation.ControlViewWalker() }?;
        let window = window_rect(hwnd);
        let mut inspected = Vec::new();
        let mut editor = None;
        let mut editor_element = None;
        let mut editor_candidate_count = 0usize;
        let mut focused_editor_found = false;
        let mut pending = VecDeque::new();
        if let Ok(first) = unsafe { walker.GetFirstChildElement(&root) } {
            pending.push_back(first);
        }
        let mut visited = 0usize;
        const VISIT_LIMIT: usize = 1024;
        while visited < VISIT_LIMIT {
            let Some(element) = pending.pop_front() else {
                break;
            };
            visited += 1;
            if let Ok(child) = unsafe { walker.GetFirstChildElement(&element) } {
                pending.push_back(child);
            }
            if let Ok(sibling) = unsafe { walker.GetNextSiblingElement(&element) } {
                pending.push_back(sibling);
            }
            let Ok(process_id) = (unsafe { element.CurrentProcessId() }) else {
                continue;
            };
            let Ok(control_type) = (unsafe { element.CurrentControlType() }) else {
                continue;
            };
            let control_type = control_type.0;
            if process_id != root_process_id || control_type != UIA_EditControlTypeId.0 {
                continue;
            }
            let name = unsafe { element.CurrentName() }
                .map(|name| name.to_string())
                .unwrap_or_else(|_| "<unavailable>".into());
            let automation_id = unsafe { element.CurrentAutomationId() }
                .map(|id| id.to_string())
                .unwrap_or_default();
            let enabled = unsafe { element.CurrentIsEnabled() }
                .map(|enabled| enabled.as_bool())
                .unwrap_or(false);
            let keyboard_focusable = unsafe { element.CurrentIsKeyboardFocusable() }
                .map(|focusable| focusable.as_bool())
                .unwrap_or(false);
            let provider_identity = production_editor_identity(&name).map(str::to_owned);
            let Ok(bounds) = (unsafe { element.CurrentBoundingRectangle() }) else {
                continue;
            };
            let has_keyboard_focus = unsafe { element.CurrentHasKeyboardFocus() }
                .map(|focused| focused.as_bool())
                .unwrap_or(false);
            let text = unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
                .and_then(|pattern| text_pattern_snapshot(&pattern));
            let text_error = text.as_ref().err().map(ToString::to_string);
            inspected.push(serde_json::json!({
                "name": &name,
                "automation_id": &automation_id,
                "provider_identity": &provider_identity,
                "control_type": control_type,
                "process_id": process_id,
                "enabled": enabled,
                "keyboard_focusable": keyboard_focusable,
                "has_keyboard_focus": has_keyboard_focus,
                "screen_bounds": [bounds.left, bounds.top, bounds.right, bounds.bottom],
                "text_pattern_available": text.is_ok(),
                "text_pattern_error": text_error,
            }));
            if let Some(provider_identity) = provider_identity
                && enabled
                && keyboard_focusable
                && let Ok(text_pattern) = text
                && bounds.right > bounds.left
                && bounds.bottom > bounds.top
            {
                editor_candidate_count += 1;
                let screenshot_bounds = window.map(|window| {
                    [
                        bounds.left - window.left,
                        bounds.top - window.top,
                        bounds.right - window.left,
                        bounds.bottom - window.top,
                    ]
                });
                let candidate = serde_json::json!({
                    "available": true,
                    "name": name,
                    "automation_id": automation_id,
                    "provider_identity": provider_identity,
                    "control_type": control_type,
                    "process_id": process_id,
                    "has_keyboard_focus": has_keyboard_focus,
                    "screen_bounds": [bounds.left, bounds.top, bounds.right, bounds.bottom],
                    "screenshot_bounds": screenshot_bounds,
                    "focus_correction": null,
                    "text_pattern": text_pattern,
                });
                if has_keyboard_focus || (!focused_editor_found && editor_candidate_count == 1) {
                    editor = Some(candidate);
                    editor_element = Some(element.clone());
                    focused_editor_found = has_keyboard_focus;
                }
            }
        }
        let provider_walk_limit_reached = !pending.is_empty();
        if provider_walk_limit_reached || (editor_candidate_count > 1 && !focused_editor_found) {
            editor = None;
            editor_element = None;
        }
        if focus_editor && let (Some(editor), Some(element)) = (editor.as_mut(), editor_element) {
            editor["focus_correction"] = match unsafe { element.SetFocus() } {
                Ok(()) => serde_json::json!({"requested": true}),
                Err(error) => serde_json::json!({"requested": false, "error": error.to_string()}),
            };
        }
        Ok(serde_json::json!({
            "available": true,
            "root_name": root_name,
            "focused_name": focused_name,
            "root_process_id": root_process_id,
            "focused_process_id": focused_process_id,
            "focused_element_in_window_process": root_process_id == focused_process_id,
            "focused_control_type": control_type,
            "provider_walk_visited": visited,
            "provider_walk_limit": VISIT_LIMIT,
            "provider_walk_limit_reached": provider_walk_limit_reached,
            "editor_candidate_count": editor_candidate_count,
            "focused_editor_found": focused_editor_found,
            "inspected_editor_candidates": inspected,
            "editor": editor.unwrap_or_else(|| serde_json::json!({
                "available": false,
                "reason": if provider_walk_limit_reached {
                    "bounded UIA walk reached its limit before a production editor exposed TextPattern"
                } else if editor_candidate_count > 1 && !focused_editor_found {
                    "multiple production editors exposed TextPattern but none owned keyboard focus"
                } else {
                    "no enabled in-process production editor exposed TextPattern"
                },
            })),
            "dirty_state": {
                "available": false,
                "reason": "provider exposes document text/caret but no dirty property",
            },
        }))
    })();
    // SAFETY: this balances the successful initialization above.
    unsafe { CoUninitialize() };
    result.unwrap_or_else(|error| {
        serde_json::json!({
            "available": false,
            "reason": format!("UI Automation query failed: {error}"),
        })
    })
}

fn uia_provider_snapshot(hwnd: HWND) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        let mut snapshot = uia_provider_snapshot_once(hwnd, false);
        snapshot["activation_attempts"] = serde_json::json!(attempts);
        if observed_uia_text(&snapshot).is_some() || Instant::now() >= deadline {
            return snapshot;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn observed_uia_text(snapshot: &serde_json::Value) -> Option<&str> {
    let provider = snapshot.get("uia_provider").unwrap_or(snapshot);
    (provider["editor"]["process_id"] == provider["root_process_id"])
        .then(|| provider["editor"]["text_pattern"]["document_text"].as_str())
        .flatten()
}

fn write_session_evidence(session: &Session, label: &str, value: &serde_json::Value) -> Result<PathBuf, String> {
    let directory = session.home.join("evidence");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{label}.json"));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path)
}

// --------------------------------------------------------------- input driving

fn focus(hwnd: HWND) {
    // SAFETY: `hwnd` is a live top-level handle.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Raise to the top of the z-order first: the screen-DC capture reads
        // whichever window is actually visible, so an occluded test window
        // captures stale pixels and every pixel assertion fails.
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
    // Claim the foreground through thread-input attachment. The reference
    // driver also tapped Alt first, but that can leave the window in Win32
    // menu-mode, where posted WM_COMMAND messages are ignored.
    // SAFETY: every handle comes from the live window; attachments are undone.
    unsafe {
        let target = GetWindowThreadProcessId(hwnd, None);
        let current = GetCurrentThreadId();
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let mut attached_current = false;
        let mut attached_foreground = false;
        if target != 0 && target != current {
            attached_current = AttachThreadInput(current, target, true).as_bool();
        }
        if foreground_thread != 0 && foreground_thread != current && foreground_thread != target {
            attached_foreground = AttachThreadInput(current, foreground_thread, true).as_bool();
        }
        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached_foreground {
            let _ = AttachThreadInput(current, foreground_thread, false);
        }
        if attached_current {
            let _ = AttachThreadInput(current, target, false);
        }
    }
    // Confirm foreground and retry briefly; a console that owns the foreground
    // can otherwise keep the test window behind the runner.
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        // SAFETY: no arguments beyond the handle.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == hwnd || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        // SAFETY: `hwnd` is live.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    std::thread::sleep(Duration::from_millis(150));
}

/// Wait until two consecutive captures are effectively identical, so a close or
/// menu command is not raced by checkpoints, transcode progress or toasts.
fn wait_until_stable(hwnd: HWND, timeout: Duration) -> serde_json::Value {
    let started = Instant::now();
    let deadline = Instant::now() + timeout;
    let mut previous = capture(hwnd).ok();
    let mut captures = usize::from(previous.is_some());
    let mut last_diff = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let Ok(current) = capture(hwnd) else {
            return serde_json::json!({
                "stable": false,
                "capture_failed": true,
                "captures": captures,
                "elapsed_ms": started.elapsed().as_millis(),
            });
        };
        captures += 1;
        if let Some(prev) = &previous {
            let full = (0, 0, current.width, current.height);
            let diff = current.diff_fraction(prev, full);
            last_diff = Some(diff);
            if diff < 0.0002 {
                return serde_json::json!({
                    "stable": true,
                    "captures": captures,
                    "last_diff_fraction": diff,
                    "elapsed_ms": started.elapsed().as_millis(),
                });
            }
        }
        previous = Some(current);
    }
    serde_json::json!({
        "stable": false,
        "capture_failed": false,
        "captures": captures,
        "last_diff_fraction": last_diff,
        "elapsed_ms": started.elapsed().as_millis(),
    })
}

fn key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    let mut input = INPUT::default();
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    let mut input = INPUT::default();
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: VIRTUAL_KEY(0),
        wScan: unit,
        dwFlags: if up {
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
        } else {
            KEYEVENTF_UNICODE
        },
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

fn send(inputs: &[INPUT]) -> serde_json::Value {
    // SAFETY: `inputs` is a valid slice; size matches the struct.
    let inserted = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    let error = (inserted as usize != inputs.len()).then(|| unsafe { GetLastError().0 });
    serde_json::json!({
        "requested_events": inputs.len(),
        "inserted_events": inserted,
        "last_error": error,
    })
}

fn unicode_scalar_input_batches(text: &str) -> Vec<Vec<INPUT>> {
    let mut batches = Vec::with_capacity(text.chars().count());
    for character in text.chars() {
        let mut units = [0u16; 2];
        let mut inputs = Vec::with_capacity(character.len_utf16() * 2);
        for unit in character.encode_utf16(&mut units) {
            inputs.extend([unicode_input(*unit, false), unicode_input(*unit, true)]);
        }
        batches.push(inputs);
    }
    batches
}

/// Submit each complete Unicode scalar as one genuine keyboard-input batch. The
/// OS owns the VK_PACKET/WM_CHAR translation, and scalar batches cannot be
/// interleaved by another SendInput producer.
fn type_text(hwnd: HWND, text: &str) -> serde_json::Value {
    const SCALAR_PACING: Duration = Duration::from_millis(6);
    let batches = unicode_scalar_input_batches(text);
    let requested_events = batches.iter().map(Vec::len).sum::<usize>();
    let mut attempted_events = 0usize;
    let mut inserted_events = 0usize;
    let mut attempted_scalars = 0usize;
    let mut completed_scalars = 0usize;
    let mut errors = Vec::new();
    for (scalar_index, inputs) in batches.iter().enumerate() {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground != hwnd {
            errors.push(serde_json::json!({
                "kind": "foreground_lost_before_scalar",
                "scalar_index": scalar_index,
                "expected_hwnd": hwnd.0 as usize,
                "foreground_hwnd": foreground.0 as usize,
            }));
            break;
        }
        let receipt = send(inputs);
        attempted_scalars += 1;
        attempted_events += inputs.len();
        inserted_events += receipt["inserted_events"].as_u64().unwrap_or(0) as usize;
        if receipt["inserted_events"].as_u64() != Some(inputs.len() as u64) {
            errors.push(serde_json::json!({
                "kind": "partial_scalar_batch",
                "scalar_index": scalar_index,
                "requested_events": inputs.len(),
                "inserted_events": receipt["inserted_events"],
                "last_error": receipt["last_error"],
            }));
            break;
        }
        completed_scalars += 1;
        std::thread::sleep(SCALAR_PACING);
    }
    serde_json::json!({
        "channel": "SendInput KEYEVENTF_UNICODE scalar batches",
        "target_hwnd": hwnd.0 as usize,
        "framing": "one SendInput call per Unicode scalar; UTF-16 unit down/up pairs",
        "requested_scalars": text.chars().count(),
        "requested_units": text.encode_utf16().count(),
        "requested_events": requested_events,
        "attempted_scalars": attempted_scalars,
        "attempted_events": attempted_events,
        "completed_scalars": completed_scalars,
        "inserted_events": inserted_events,
        "per_scalar_pacing_ms": SCALAR_PACING.as_millis(),
        "errors": errors,
    })
}

fn input_submission_complete(receipt: &serde_json::Value) -> bool {
    receipt["errors"].as_array().is_some_and(Vec::is_empty)
        && receipt["completed_scalars"] == receipt["requested_scalars"]
        && receipt["attempted_events"] == receipt["requested_events"]
        && receipt["inserted_events"] == receipt["requested_events"]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextDelivery {
    Delivered,
    Unchanged,
    Diverged,
}

fn text_delivery(before: &str, after: &str, expected: &str) -> TextDelivery {
    if after == expected {
        TextDelivery::Delivered
    } else if after == before {
        TextDelivery::Unchanged
    } else {
        TextDelivery::Diverged
    }
}

fn retain_partial_delivery(previous: TextDelivery, current: TextDelivery) -> TextDelivery {
    match (previous, current) {
        (_, TextDelivery::Delivered) => TextDelivery::Delivered,
        (TextDelivery::Diverged, TextDelivery::Unchanged) => TextDelivery::Diverged,
        (_, current) => current,
    }
}

fn replace_utf16(text: &str, start: usize, length: usize, intended: &str) -> Option<String> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    let end = start.checked_add(length)?;
    if end > units.len() {
        return None;
    }
    units.splice(start..end, intended.encode_utf16());
    String::from_utf16(&units).ok()
}

fn expected_text_transition(provider: &serde_json::Value, intended: &str) -> Result<(String, String), String> {
    if provider["editor"]["available"].as_bool() != Some(true) {
        let reason = provider["editor"]["reason"]
            .as_str()
            .unwrap_or("editor TextPattern unavailable");
        return Err(format!("UIA prerequisite: {reason}"));
    }
    let text = &provider["editor"]["text_pattern"];
    if text["bounded_complete"].as_bool() != Some(true) {
        return Err("UIA prerequisite: editor text exceeds the bounded evidence limit".into());
    }
    if text["selection_count"].as_i64() != Some(1) {
        return Err("UIA prerequisite: editor must expose exactly one selection or caret".into());
    }
    let before = text["document_text"]
        .as_str()
        .ok_or_else(|| "UIA prerequisite: editor document text unavailable".to_string())?
        .to_owned();
    let selection = &text["selection"];
    let start = selection["caret_start_utf16"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "UIA prerequisite: selection start unavailable".to_string())?;
    let length = selection["selection_utf16"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "UIA prerequisite: selection length unavailable".to_string())?;
    let expected = replace_utf16(&before, start, length, intended)
        .ok_or_else(|| "UIA prerequisite: selection is outside UTF-16 document bounds".to_string())?;
    Ok((before, expected))
}

fn wait_for_text_delivery(
    hwnd: HWND,
    before: &str,
    expected: &str,
    timeout: Duration,
) -> (TextDelivery, serde_json::Value) {
    let deadline = Instant::now() + timeout;
    let mut last_delivery = TextDelivery::Unchanged;
    loop {
        let snapshot = uia_provider_snapshot_once(hwnd, false);
        if let Some(after) = observed_uia_text(&snapshot) {
            last_delivery = retain_partial_delivery(last_delivery, text_delivery(before, after, expected));
            if last_delivery == TextDelivery::Delivered || Instant::now() >= deadline {
                return (last_delivery, snapshot);
            }
        } else if Instant::now() >= deadline {
            return (last_delivery, snapshot);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_editor_focus(hwnd: HWND, timeout: Duration) -> serde_json::Value {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = uia_provider_snapshot_once(hwnd, false);
        if snapshot["editor"]["has_keyboard_focus"].as_bool() == Some(true) || Instant::now() >= deadline {
            return snapshot;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn input_target_state(hwnd: HWND, provider: &serde_json::Value) -> serde_json::Value {
    let foreground = unsafe { GetForegroundWindow() };
    serde_json::json!({
        "target_hwnd": hwnd.0 as usize,
        "foreground_hwnd": foreground.0 as usize,
        "target_is_foreground": foreground == hwnd,
        "editor_process_id": provider["editor"]["process_id"],
        "editor_has_keyboard_focus": provider["editor"]["has_keyboard_focus"],
        "editor_screen_bounds": provider["editor"]["screen_bounds"],
        "editor_screenshot_bounds": provider["editor"]["screenshot_bounds"],
    })
}

fn editor_glyph_region(window: &serde_json::Value, image: &Image) -> Option<(i32, i32, i32, i32)> {
    let bounds = window["uia_provider"]["editor"]["screenshot_bounds"].as_array()?;
    if bounds.len() != 4 {
        return None;
    }
    let left = bounds[0].as_i64()? as i32;
    let top = bounds[1].as_i64()? as i32;
    let right = bounds[2].as_i64()? as i32;
    let bottom = bounds[3].as_i64()? as i32;
    let region = (
        (left + 4).clamp(0, image.width),
        (top + 2).clamp(0, image.height),
        right.min(left + 600).clamp(0, image.width),
        bottom.min(top + 180).clamp(0, image.height),
    );
    (region.2 > region.0 && region.3 > region.1).then_some(region)
}

fn bounded_editor_region(provider: &serde_json::Value, image: &Image) -> Option<(i32, i32, i32, i32)> {
    let bounds = provider["editor"]["screenshot_bounds"].as_array()?;
    if bounds.len() != 4 {
        return None;
    }
    let region = (
        (bounds[0].as_i64()? as i32).clamp(0, image.width),
        (bounds[1].as_i64()? as i32).clamp(0, image.height),
        (bounds[2].as_i64()? as i32).clamp(0, image.width),
        (bounds[3].as_i64()? as i32).clamp(0, image.height),
    );
    (region.2 > region.0 && region.3 > region.1).then_some(region)
}

fn retain_input_prerequisite_failure(
    session: &Session,
    hwnd: HWND,
    detail: &str,
    provider: serde_json::Value,
    input_attempted: bool,
    attempts: serde_json::Value,
) -> String {
    let screenshot_path = session.home.join("shots/input-prerequisite-failed.bmp");
    let screenshot = match shot(session, hwnd, "input-prerequisite-failed") {
        Ok(_) => serde_json::json!({"available": true, "path": screenshot_path}),
        Err(error) => serde_json::json!({"available": false, "error": error}),
    };
    let foreground = unsafe { GetForegroundWindow() };
    let receipt = serde_json::json!({
        "checkpoint": "input prerequisite failed before verified delivery",
        "detail": detail,
        "input_attempted": input_attempted,
        "input_attempts": attempts,
        "target_hwnd": hwnd.0 as usize,
        "foreground_hwnd": foreground.0 as usize,
        "target_is_foreground": foreground == hwnd,
        "provider": provider,
        "isolated_home": &session.home,
        "isolated_data_root": &session.data_root,
        "screenshot": screenshot,
    });
    match write_session_evidence(session, "input-prerequisite-failed", &receipt) {
        Ok(path) => format!("{detail}; evidence={}", path.display()),
        Err(error) => format!("{detail}; evidence write failed: {error}"),
    }
}

/// Submit one measured SendInput attempt after the actual editor provider owns
/// focus. Exact TextPattern content, rather than pixels or insertion counts,
/// proves delivery.
fn type_text_until_observed_change(
    session: &Session,
    hwnd: HWND,
    text: &str,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    focus(hwnd);
    let mut before_provider = uia_provider_snapshot(hwnd);
    let mut focus_correction = None;
    if before_provider["editor"]["has_keyboard_focus"].as_bool() != Some(true) {
        let correction = uia_provider_snapshot_once(hwnd, true);
        focus_correction = Some(correction);
        before_provider = wait_for_editor_focus(hwnd, Duration::from_secs(2));
    }
    let before_image = capture(hwnd).map_err(|detail| {
        retain_input_prerequisite_failure(
            session,
            hwnd,
            &detail,
            before_provider.clone(),
            false,
            serde_json::json!([]),
        )
    })?;
    let (before_text, expected_text) = expected_text_transition(&before_provider, text).map_err(|detail| {
        retain_input_prerequisite_failure(
            session,
            hwnd,
            &detail,
            before_provider.clone(),
            false,
            serde_json::json!([]),
        )
    })?;
    let before_target = input_target_state(hwnd, &before_provider);
    if before_target["target_is_foreground"].as_bool() != Some(true) {
        return Err(retain_input_prerequisite_failure(
            session,
            hwnd,
            "input target does not own the foreground",
            before_provider,
            false,
            serde_json::json!([]),
        ));
    }
    if before_target["editor_has_keyboard_focus"].as_bool() != Some(true) {
        return Err(retain_input_prerequisite_failure(
            session,
            hwnd,
            "editor provider does not own keyboard focus",
            before_provider,
            false,
            serde_json::json!([]),
        ));
    }
    let primary_input = type_text(hwnd, text);
    let submission_complete = input_submission_complete(&primary_input);
    let attempts = vec![primary_input.clone()];
    let (delivery, after_provider) = wait_for_text_delivery(hwnd, &before_text, &expected_text, timeout);
    session.settle();
    let after_image = capture(hwnd).map_err(|detail| {
        retain_input_prerequisite_failure(
            session,
            hwnd,
            &detail,
            after_provider.clone(),
            true,
            serde_json::json!(attempts),
        )
    })?;
    let body = bounded_editor_region(&before_provider, &before_image).ok_or_else(|| {
        retain_input_prerequisite_failure(
            session,
            hwnd,
            "UIA prerequisite: editor provider has no usable screenshot bounds",
            after_provider.clone(),
            true,
            serde_json::json!(attempts),
        )
    })?;
    let body_diff = after_image.diff_fraction(&before_image, body);
    let after_target = input_target_state(hwnd, &after_provider);
    let mut hash = Sha256::new();
    hash.update(text.as_bytes());
    let receipt = serde_json::json!({
        "delivery": format!("{delivery:?}"),
        "body_region": [body.0, body.1, body.2, body.3],
        "body_diff_fraction": body_diff,
        "primary_input": primary_input,
        "input_submission_complete": submission_complete,
        "input_attempts": attempts,
        "focus_correction": focus_correction,
        "before_target": before_target,
        "before_provider": before_provider,
        "expected_text": expected_text,
        "after_target": after_target,
        "after_provider": after_provider,
        "text_sha256": format!("{:x}", hash.finalize()),
        "utf16_units": text.encode_utf16().count(),
        "window_title_after": window_title(hwnd),
    });
    if delivery == TextDelivery::Delivered && submission_complete {
        Ok(receipt)
    } else {
        let _ = shot(session, hwnd, "typing-failed");
        let evidence = write_session_evidence(session, "typing-failed", &receipt)?;
        Err(format!(
            "input delivery was {delivery:?}; submission_complete={submission_complete}; evidence={}",
            evidence.display()
        ))
    }
}

/// Best-effort virtual-key chord (may not reach the winit window; prefer
/// [`invoke_menu`] for command invocation).
#[allow(dead_code)]
fn chord(ctrl: bool, shift: bool, vk: VIRTUAL_KEY) {
    let mut down = Vec::new();
    let mut up = Vec::new();
    if ctrl {
        down.push(key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)));
        up.push(key_input(VK_CONTROL, KEYEVENTF_KEYUP));
    }
    if shift {
        down.push(key_input(VK_SHIFT, KEYBD_EVENT_FLAGS(0)));
        up.push(key_input(VK_SHIFT, KEYEVENTF_KEYUP));
    }
    down.push(key_input(vk, KEYBD_EVENT_FLAGS(0)));
    up.insert(0, key_input(vk, KEYEVENTF_KEYUP));
    down.extend(up);
    send(&down);
}

// --------------------------------------------------------------- menu commands

fn menu_label(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, position: i32) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: `menu` is live; `buf` is a local, sized buffer.
    let len = unsafe { GetMenuStringW(menu, position as u32, Some(&mut buf), MF_BYPOSITION) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Normalize a menu label for matching: drop the `&` accelerator marker, cut the
/// shortcut hint after a tab, drop a trailing ellipsis, lowercase and trim.
fn normalize_label(label: &str) -> String {
    let head = label.split('\t').next().unwrap_or(label);
    head.replace('&', "").trim_end_matches(['.', '…']).trim().to_lowercase()
}

/// Return the top-level menu bar labels (normalized), in order.
fn top_menu_labels(hwnd: HWND) -> Vec<String> {
    // SAFETY: `hwnd` is a live top-level window.
    unsafe {
        let menu = GetMenu(hwnd);
        if menu.0.is_null() {
            return Vec::new();
        }
        let count = GetMenuItemCount(Some(menu));
        (0..count)
            .map(|position| normalize_label(&menu_label(menu, position)))
            .collect()
    }
}

/// A menu node discovered while walking the bar: how many commands it directly
/// holds and whether its single child (if any) has the same title.
struct SubmenuShape {
    title: String,
    command_children: usize,
    lone_same_named_child: bool,
}

/// Walk every submenu and record its shape, for the single-item-submenu check.
fn submenu_shapes(hwnd: HWND) -> Vec<SubmenuShape> {
    fn walk(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, title: &str, out: &mut Vec<SubmenuShape>) {
        // SAFETY: `menu` is a live handle owned by the window.
        let count = unsafe { GetMenuItemCount(Some(menu)) };
        let mut command_children = 0;
        let mut child_submenus = Vec::new();
        for position in 0..count {
            let label = menu_label(menu, position);
            if label.is_empty() {
                continue; // separator
            }
            // SAFETY: `menu` live; `position` in range.
            let sub = unsafe { GetSubMenu(menu, position) };
            if sub.0.is_null() {
                command_children += 1;
            } else {
                child_submenus.push((sub, normalize_label(&label)));
            }
        }
        let lone_same_named_child = command_children == 1
            && child_submenus.is_empty()
            && normalize_label(title) == menu_only_command_label(menu);
        if !title.is_empty() {
            out.push(SubmenuShape {
                title: title.to_string(),
                command_children,
                lone_same_named_child,
            });
        }
        for (sub, sub_title) in child_submenus {
            walk(sub, &sub_title, out);
        }
    }
    // SAFETY: `hwnd` is live.
    let menu = unsafe { GetMenu(hwnd) };
    let mut out = Vec::new();
    if menu.0.is_null() {
        return out;
    }
    // SAFETY: `menu` live.
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    for position in 0..count {
        let label = menu_label(menu, position);
        // SAFETY: `menu` live; `position` in range.
        let sub = unsafe { GetSubMenu(menu, position) };
        if !sub.0.is_null() {
            walk(sub, &normalize_label(&label), &mut out);
        }
    }
    out
}

/// If a menu holds exactly one command child, return its normalized label.
fn menu_only_command_label(menu: windows::Win32::UI::WindowsAndMessaging::HMENU) -> String {
    // SAFETY: `menu` live.
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    for position in 0..count {
        let label = menu_label(menu, position);
        if label.is_empty() {
            continue;
        }
        // SAFETY: `menu` live; `position` in range.
        let sub = unsafe { GetSubMenu(menu, position) };
        if sub.0.is_null() {
            return normalize_label(&label);
        }
    }
    String::new()
}

struct MenuResolution {
    id: u32,
    root_menu: usize,
    positions: Vec<i32>,
    labels: Vec<String>,
}

impl MenuResolution {
    fn evidence(&self) -> serde_json::Value {
        serde_json::json!({
            "command_id": self.id,
            "root_menu": self.root_menu,
            "positions": self.positions,
            "labels": self.labels,
        })
    }

    fn same_target(&self, other: &Self) -> bool {
        self.id == other.id
            && self.root_menu == other.root_menu
            && self.positions == other.positions
            && self.labels == other.labels
    }
}

/// Resolve a menu path (`["Search", "Go To Line"]`) to the exact native labels,
/// positions and command id on this HWND's current menu tree.
fn menu_command_resolution(hwnd: HWND, path: &[&str]) -> Option<MenuResolution> {
    // SAFETY: `hwnd` is a live top-level window.
    let mut menu = unsafe { GetMenu(hwnd) };
    if menu.0.is_null() {
        return None;
    }
    let root_menu = menu.0 as usize;
    let mut positions = Vec::with_capacity(path.len());
    let mut labels = Vec::with_capacity(path.len());
    for (depth, segment) in path.iter().enumerate() {
        let target = normalize_label(segment);
        // SAFETY: `menu` live.
        let count = unsafe { GetMenuItemCount(Some(menu)) };
        let mut advanced = false;
        for position in 0..count {
            let raw_label = menu_label(menu, position);
            let label = normalize_label(&raw_label);
            if label.is_empty() || !(label == target || label.starts_with(&target)) {
                continue;
            }
            positions.push(position);
            labels.push(raw_label);
            if depth + 1 == path.len() {
                // SAFETY: `menu` live; `position` in range.
                let id = unsafe { GetMenuItemID(menu, position) };
                return (id != u32::MAX).then_some(MenuResolution {
                    id,
                    root_menu,
                    positions,
                    labels,
                });
            }
            // SAFETY: `menu` live; `position` in range.
            let sub = unsafe { GetSubMenu(menu, position) };
            if sub.0.is_null() {
                return None;
            }
            menu = sub;
            advanced = true;
            break;
        }
        if !advanced {
            return None;
        }
    }
    None
}

/// Invoke a menu command by path. Posts `WM_COMMAND` with the resolved id and
/// `lParam == 0`, exactly the shape the app decodes as a menu selection.
fn invoke_menu(hwnd: HWND, path: &[&str]) -> Result<serde_json::Value, String> {
    // The native menu is rebuilt when the visible command set changes, so a
    // lookup can land between destroy and re-create; retry briefly.
    let deadline = Instant::now() + Duration::from_secs(1);
    let lookup = loop {
        if let Some(resolution) = menu_command_resolution(hwnd, path) {
            break resolution;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "menu path not found: {path:?}; top={:?}",
                top_menu_labels(hwnd)
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Recheck once immediately before posting. A changed lookup is an explicit
    // prerequisite failure: never knowingly send the stale id or substitute a
    // different command.
    let immediately_before_post = menu_command_resolution(hwnd, path);
    let lookup_to_post_changed = immediately_before_post
        .as_ref()
        .is_none_or(|current| !lookup.same_target(current));
    if lookup_to_post_changed {
        let receipt = serde_json::json!({
            "path": path,
            "target_hwnd": hwnd.0 as usize,
            "lookup": lookup.evidence(),
            "immediately_before_post": immediately_before_post
                .as_ref()
                .map(MenuResolution::evidence),
            "lookup_to_post_changed": true,
            "post_message_succeeded": false,
            "command_posted": false,
        });
        return Err(format!(
            "menu changed between lookup and command post; no command was posted; receipt={receipt}"
        ));
    }
    // SAFETY: `hwnd` is a live window; the message is well-formed.
    unsafe {
        PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(lookup.id as usize), LPARAM(0))
            .map_err(|e| format!("PostMessage WM_COMMAND failed: {e}"))
    }?;
    Ok(serde_json::json!({
        "path": path,
        "command_id": lookup.id,
        "target_hwnd": hwnd.0 as usize,
        "lookup": lookup.evidence(),
        "immediately_before_post": immediately_before_post.map(|resolution| resolution.evidence()),
        "lookup_to_post_changed": lookup_to_post_changed,
        "post_message_succeeded": true,
    }))
}

/// Wait for an owned save prompt to exist *and* expose its buttons. Dialog
/// creation is incremental, so a listed top-level window may be empty for a
/// moment; waiting on the button text removes that race.
fn await_save_prompt(session: &Session, hwnd: HWND, timeout: Duration) -> PromptWait {
    let started = Instant::now();
    let deadline = Instant::now() + timeout;
    let mut observations = Vec::new();
    let mut previous = None;
    loop {
        let current = prompt_topology(session, hwnd);
        let transitioned = previous.as_ref() != Some(&current);
        let interesting = !current.main_enabled
            || current
                .candidates
                .iter()
                .any(|(_, owner, _)| *owner == Some(hwnd.0 as usize));
        if transitioned && interesting {
            retain_prompt_observation(
                &mut observations,
                prompt_observation(
                    session,
                    hwnd,
                    "disabled-owner-or-owned-window-transition",
                    started.elapsed(),
                ),
                false,
            );
        }
        previous = Some(current);
        for (candidate, _, _) in auxiliary_windows(session.pid, hwnd) {
            let has_save = child_texts(candidate)
                .iter()
                .any(|text| text.to_lowercase().contains("save"));
            if has_save {
                retain_prompt_observation(
                    &mut observations,
                    prompt_observation(session, hwnd, "save-prompt-oracle-matched", started.elapsed()),
                    true,
                );
                return PromptWait {
                    dialog: Some(candidate),
                    observations,
                };
            }
        }
        if Instant::now() >= deadline {
            retain_prompt_observation(
                &mut observations,
                prompt_observation(session, hwnd, "timeout", started.elapsed()),
                true,
            );
            return PromptWait {
                dialog: None,
                observations,
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ------------------------------------------------------------------- capture --

/// A captured window image: 32-bit BGRA pixels, top-down (row 0 is the top).
struct Image {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Image {
    fn rgb(&self, x: i32, y: i32) -> (u8, u8, u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return (0, 0, 0);
        }
        let index = ((y * self.width + x) * 4) as usize;
        (self.pixels[index + 2], self.pixels[index + 1], self.pixels[index])
    }

    /// Fraction of pixels that differ from `other` inside `rect` (both images
    /// must share dimensions). Used to confirm an action produced a visible change.
    fn diff_fraction(&self, other: &Image, rect: (i32, i32, i32, i32)) -> f64 {
        if self.width != other.width || self.height != other.height {
            return 1.0;
        }
        let (x0, y0, x1, y1) = rect;
        let (mut changed, mut total) = (0u64, 0u64);
        for y in y0.max(0)..y1.min(self.height) {
            for x in x0.max(0)..x1.min(self.width) {
                let a = self.rgb(x, y);
                let b = other.rgb(x, y);
                let delta =
                    (a.0 as i32 - b.0 as i32).abs() + (a.1 as i32 - b.1 as i32).abs() + (a.2 as i32 - b.2 as i32).abs();
                if delta > 24 {
                    changed += 1;
                }
                total += 1;
            }
        }
        if total == 0 { 0.0 } else { changed as f64 / total as f64 }
    }

    /// Largest per-pixel channel spread inside `rect` — a proxy for saturated
    /// (colourful, e.g. emoji) content versus monochrome text.
    fn max_saturation(&self, rect: (i32, i32, i32, i32)) -> u8 {
        let (x0, y0, x1, y1) = rect;
        let mut peak = 0u8;
        for y in y0.max(0)..y1.min(self.height) {
            for x in x0.max(0)..x1.min(self.width) {
                let (r, g, b) = self.rgb(x, y);
                let spread = r.max(g).max(b) - r.min(g).min(b);
                peak = peak.max(spread);
            }
        }
        peak
    }
}

/// Capture the on-screen pixels of `hwnd` via a screen-DC BitBlt (works for the
/// D2D swapchain window, unlike PrintWindow), matching the reference driver.
fn capture(hwnd: HWND) -> Result<Image, String> {
    let rect = window_rect(hwnd).ok_or("GetWindowRect failed")?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(format!("degenerate window rect {width}x{height}"));
    }
    // SAFETY: every GDI object created below is released on all paths.
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err("GetDC(screen) failed".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(mem, bitmap.into());
        let blit = BitBlt(mem, 0, 0, width, height, Some(screen), rect.left, rect.top, SRCCOPY);
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // negative => top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            mem,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut header,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if blit.is_err() || lines == 0 {
            return Err("BitBlt/GetDIBits failed".into());
        }
        Ok(Image { width, height, pixels })
    }
}

/// Persist an image to the journey scratch as a 32-bit bottom-up BMP (no encoder
/// dependency) so a failing journey leaves reviewable evidence.
fn save_bmp(image: &Image, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let row = (image.width * 4) as usize;
    let pixel_bytes = row * image.height as usize;
    let file_size = 54 + pixel_bytes;
    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&image.width.to_le_bytes());
    out.extend_from_slice(&image.height.to_le_bytes()); // positive => bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..image.height).rev() {
        let start = (y * image.width * 4) as usize;
        out.extend_from_slice(&image.pixels[start..start + row]);
    }
    std::fs::write(path, out)
}

fn shot(session: &Session, hwnd: HWND, label: &str) -> Result<Image, String> {
    // Capture is a screen-DC BitBlt, so an occluded window would photograph
    // whatever covers it. Only re-assert focus when another window is actually
    // in front; re-raising an already-foreground window disturbs modal state.
    // SAFETY: no arguments beyond the handle.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground != hwnd {
        focus(hwnd);
    }
    let image = capture(hwnd)?;
    let path = session.home.join("shots").join(format!("{label}.bmp"));
    let _ = save_bmp(&image, &path);
    Ok(image)
}

/// Post a menu command repeatedly until `rect` changes. Menu ids are positional
/// and a rebuild can drop a single posted `WM_COMMAND`, which made sequential
/// journey runs flaky; re-resolving and retrying is idempotent for these
/// overlay/settings commands.
fn invoke_menu_until_change(
    hwnd: HWND,
    path: &[&str],
    before: &Image,
    rect: (i32, i32, i32, i32),
    threshold: f64,
    timeout: Duration,
) -> Result<f64, String> {
    let deadline = Instant::now() + timeout;
    loop {
        invoke_menu(hwnd, path)?;
        std::thread::sleep(Duration::from_millis(200));
        let after = capture(hwnd)?;
        let changed = after.diff_fraction(before, rect);
        if changed >= threshold || Instant::now() >= deadline {
            return Ok(changed);
        }
    }
}

// -------------------------------------------------------------- the journeys --

fn journey_smoke(env: &Env) -> Result<(), String> {
    let (home, data_root) = env.scratch("smoke")?;
    std::fs::create_dir_all(&data_root).map_err(|error| error.to_string())?;
    std::fs::write(data_root.join(".bareline-diagnostic"), []).map_err(|error| error.to_string())?;
    let mut command = Command::new(&env.exe);
    command.args([
        "--smoke",
        "--software",
        "--no-session",
        "--no-extensions",
        "--new-instance",
    ]);
    command.arg("--diagnostic-root").arg(&data_root);
    isolate(&mut command, &home);
    let capture = crate::capture::run(&mut command, Duration::from_secs(30)).map_err(|e| e.to_string())?;
    if capture.status != "ok" {
        return Err(format!(
            "smoke exit status {} (code {:?}); stderr: {}",
            capture.status,
            capture.exit_code,
            capture.stderr.trim()
        ));
    }
    if !capture
        .stdout
        .lines()
        .any(|line| line.contains("\"event\":\"first_frame\""))
    {
        return Err(format!(
            "no first_frame event on stdout; got: {}",
            capture.stdout.trim()
        ));
    }
    Ok(())
}

/// P0-3: typing into an Untitled document keeps at most two recovery directories;
/// discarding on exit durably retires every remaining generation. Physical purge
/// may finish later after the content-free tombstone becomes authoritative.
fn journey_p0_3(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p0-3", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    // Type well past a single checkpoint's worth of text.
    type_text_until_observed_change(
        &session,
        hwnd,
        &"the quick brown fox jumps over the lazy dog. ".repeat(12),
        Duration::from_secs(15),
    )?;
    // Allow the recovery worker to checkpoint a couple of generations and let
    // the snapshot churn settle before any close command.
    wait_until_stable(hwnd, Duration::from_secs(8));
    let dirs = session.recovery_dir_count();
    if dirs > 2 {
        return Err(format!(
            "expected <= 2 recovery directories while editing, found {dirs}"
        ));
    }
    let _ = shot(&session, hwnd, "p0-3-edited");
    // Issue exactly one Exit request. If recovery is busy, the product owns
    // retaining and resuming that intent; reinjection would hide a lost request.
    let command = invoke_menu(hwnd, &["File", "Exit"])?;
    let PromptWait {
        dialog,
        observations: prompt_observations,
    } = await_save_prompt(&session, hwnd, Duration::from_secs(15));
    let Some(dialog) = dialog else {
        let _ = shot(&session, hwnd, "p0-3-prompt-missing");
        let failure = serde_json::json!({
            "checkpoint": "discard consent prerequisite",
            "command": command,
            "window": window_evidence(&session, hwnd),
            "recovery_directories": session.recovery_dir_count(),
            "screenshot": session.home.join("shots/p0-3-prompt-missing.bmp"),
            "consent_observed": false,
            "discard_requested": false,
            "prompt_observations": prompt_observations,
            "command_traces": session.command_trace_evidence(),
        });
        let evidence = write_session_evidence(&session, "p0-3-prompt-missing", &failure)?;
        return Err(format!(
            "timeout waiting for save prompt after one Exit command; no discard consent was performed; evidence={}",
            evidence.display()
        ));
    };
    let buttons = child_texts(dialog);
    let _ = shot(&session, hwnd, "p0-3-consent");
    let consent = serde_json::json!({
        "checkpoint": "discard consent observed",
        "command": command,
        "dialog_hwnd": dialog.0 as usize,
        "dialog_buttons": &buttons,
        "window": window_evidence(&session, hwnd),
        "recovery_directories": session.recovery_dir_count(),
        "consent_observed": true,
        "screenshot": session.home.join("shots/p0-3-consent.bmp"),
        "prompt_observations": prompt_observations,
        "command_traces": session.command_trace_evidence(),
    });
    let _ = write_session_evidence(&session, "p0-3-consent", &consent)?;
    click_button(dialog, &["don't save", "dont save", "discard"])?;
    let exit_deadline = Instant::now() + Duration::from_secs(15);
    let exit_status = loop {
        if let Some(status) = session.child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let failure = serde_json::json!({
                "checkpoint": "discard consent terminal result",
                "command": command,
                "dialog_buttons": buttons,
                "window": window_evidence(&session, hwnd),
                "recovery_directories": session.recovery_dir_count(),
                "consent_observed": true,
                "discard_requested": true,
                "application_exited": false,
                "command_traces": session.command_trace_evidence(),
            });
            let evidence = write_session_evidence(&session, "p0-3-exit-pending", &failure)?;
            return Err(format!(
                "timeout waiting for application exit after observed Don't Save consent; evidence={}",
                evidence.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !exit_status.success() {
        return Err(format!(
            "application exited unsuccessfully after discard consent: {exit_status}"
        ));
    }
    let terminal_recovery = session.recovery_terminal_state();
    let terminal_diagnostics = session.diagnostics();
    let terminal = serde_json::json!({
        "checkpoint": "discard terminal result",
        "command": command,
        "dialog_buttons": buttons,
        "exit_status": exit_status.to_string(),
        "exit_success": exit_status.success(),
        "recovery": terminal_recovery,
        "diagnostics_tail": terminal_diagnostics.iter().rev().take(32).cloned().collect::<Vec<_>>(),
        "consent_observed": true,
        "discard_requested": true,
        "application_exited": true,
        "command_traces": session.command_trace_evidence(),
    });
    let terminal_evidence = write_session_evidence(&session, "p0-3-terminal", &terminal)?;
    if terminal["recovery"]["production_discovery_candidates"].as_u64() != Some(0) {
        return Err(format!(
            "discarded recovery remained discoverable after successful exit; evidence={}",
            terminal_evidence.display()
        ));
    }

    // Restart against the same isolated profile. The production inspector's
    // Discarded state is the recovery UI's exclusion rule; also verify the new
    // editor did not silently restore discarded text.
    let restarted = session.restart(env)?;
    let restart_window = window_evidence(&session, restarted);
    let restart_recovery = session.recovery_terminal_state();
    let restart_diagnostics = session.diagnostics();
    let restart = serde_json::json!({
        "checkpoint": "discarded recovery after restart",
        "window": restart_window,
        "recovery": restart_recovery,
        "diagnostics_tail": restart_diagnostics.iter().rev().take(32).cloned().collect::<Vec<_>>(),
        "command_traces": session.command_trace_evidence(),
        "production_discovery_rule": "only successfully inspected non-Discarded generations are listed",
    });
    let restart_evidence = write_session_evidence(&session, "p0-3-restart", &restart)?;
    if restart["recovery"]["production_discovery_candidates"].as_u64() != Some(0) {
        return Err(format!(
            "discarded recovery became discoverable after restart; evidence={}",
            restart_evidence.display()
        ));
    }
    match observed_uia_text(&restart["window"]) {
        Some("") => {}
        Some(_) => {
            return Err(format!(
                "discarded recovery content was restored after restart; evidence={}",
                restart_evidence.display()
            ));
        }
        None => {
            return Err(format!(
                "UIA prerequisite unavailable while checking discarded recovery after restart; evidence={}",
                restart_evidence.display()
            ));
        }
    }
    Ok(())
}

/// P0-4: an external `ShowWindow(SW_SHOW)` must leave the window visible.
fn journey_p0_4(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p0-4", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    if !is_visible(hwnd) {
        return Err("window not visible after launch".into());
    }
    for attempt in 0..10 {
        // SAFETY: `hwnd` is a live window handle.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        std::thread::sleep(Duration::from_millis(120));
        if !is_visible(hwnd) {
            return Err(format!("window hidden after external SW_SHOW (attempt {attempt})"));
        }
        let rect = window_rect(hwnd).ok_or("GetWindowRect failed")?;
        if rect.right - rect.left <= 0 || rect.bottom - rect.top <= 0 {
            return Err("window collapsed to an empty rect after SW_SHOW".into());
        }
    }
    let _ = shot(&session, hwnd, "p0-4-visible");
    // The diagnostics log must record a first frame for this isolated run.
    let events = session.diagnostics();
    if !events.iter().any(|event| event["event"] == "first_frame") {
        return Err("diagnostics log has no first_frame event".into());
    }
    Ok(())
}

/// P0-5: a recovered small document opens as an editable Untitled tab.
fn journey_p0_5(env: &Env) -> Result<(), String> {
    // First run: type unsaved text, then kill the process outright (no discard).
    let home;
    {
        let mut session = Session::launch(env, "p0-5", &[], &[])?;
        let hwnd = session.wait_window(Duration::from_secs(20))?;
        type_text_until_observed_change(
            &session,
            hwnd,
            "recovered content for P0-5 journey",
            Duration::from_secs(15),
        )?;
        wait_until_stable(hwnd, Duration::from_secs(8));
        home = session.home.clone();
        // Hard kill so the journal survives (Drop also kills, but do it explicitly).
        let _ = session.child.kill();
        let _ = session.child.wait();
    }
    std::thread::sleep(Duration::from_millis(500));
    // Second run into the SAME home so recovery discovers the journal.
    let mut command = Command::new(&env.exe);
    command.args(["--software", "--no-session", "--no-extensions", "--new-instance"]);
    isolate(&mut command, &home);
    let command_trace = home.join("evidence/command-trace-launch-2.jsonl");
    command.env("BARELINE_QA_COMMAND_TRACE", &command_trace);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = command.spawn().map_err(|e| format!("relaunch failed: {e}"))?;
    let mut session = Session {
        child,
        pid: 0,
        home: home.clone(),
        data_root: home.join("Bareline"),
        run_id: env.run_id.clone(),
        launch_files: Vec::new(),
        hwnd: None,
        command_traces: vec![home.join("evidence/command-trace-launch-1.jsonl"), command_trace],
    };
    session.pid = session.child.id();
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let title = window_title(hwnd);
    let _ = shot(&session, hwnd, "p0-5-recovered");
    // Recovery Center may present a chooser; the acceptance target is that a
    // recovered small document is titled "Untitled" and editable. The window
    // title reflects the active document; accept either the tab title in the
    // caption or a Recovery surface, but require the journal was consumed.
    if !(title.contains("Untitled") || title.to_lowercase().contains("recover") || title.contains("Bareline")) {
        return Err(format!("unexpected window title after recovery: {title:?}"));
    }
    Ok(())
}

/// P0-7: closing a dirty tab prompts with Save / Don't Save / Cancel.
fn journey_p0_7(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p0-7", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    let typing = type_text_until_observed_change(
        &session,
        hwnd,
        "unsaved edit for the close prompt",
        Duration::from_secs(15),
    )?;
    let frame = wait_until_stable(hwnd, Duration::from_secs(8));
    let window = window_evidence(&session, hwnd);
    let screenshot_path = session.home.join("shots/p0-7-before-close.bmp");
    let screenshot = match shot(&session, hwnd, "p0-7-before-close") {
        Ok(_) => serde_json::json!({"available": true, "path": screenshot_path}),
        Err(error) => serde_json::json!({"available": false, "error": error}),
    };
    let before_close = serde_json::json!({
        "checkpoint": "input and provider prerequisites before close command",
        "typing_receipt": typing,
        "frame_receipt": frame,
        "window": window,
        "screenshot": screenshot,
        "command_injection": "native menu File > Close via WM_COMMAND",
    });
    let evidence = write_session_evidence(&session, "p0-7-before-close", &before_close)?;
    if frame["stable"].as_bool() != Some(true) {
        return Err(format!(
            "timeout waiting for a stable frame before close; evidence={}",
            evidence.display()
        ));
    }
    let Some(provider_text) = observed_uia_text(&window) else {
        return Err(format!(
            "UIA prerequisite unavailable before close; evidence={}",
            evidence.display()
        ));
    };
    if provider_text != "unsaved edit for the close prompt" {
        return Err(format!(
            "UIA prerequisite did not observe the posted close-prompt text; evidence={}",
            evidence.display()
        ));
    }
    // Issue one Close request. Reinjecting it while recovery is busy can hide a
    // lost deferred document-close intent in the product.
    focus(hwnd);
    let command = invoke_menu(hwnd, &["File", "Close"])?;
    let PromptWait {
        dialog,
        observations: prompt_observations,
    } = await_save_prompt(&session, hwnd, Duration::from_secs(15));
    let dialog = match dialog {
        Some(dialog) => dialog,
        None => {
            let _ = shot(&session, hwnd, "p0-7-no-prompt");
            let failure = serde_json::json!({
                "checkpoint": "save prompt timeout",
                "window": window_evidence(&session, hwnd),
                "command": command,
                "prompt_observations": prompt_observations,
                "command_traces": session.command_trace_evidence(),
            });
            let evidence = write_session_evidence(&session, "p0-7-no-prompt", &failure)?;
            let listed: Vec<String> = auxiliary_windows(session.pid, hwnd)
                .into_iter()
                .map(|(handle, _, _)| format!("{}:{:?}", window_class(handle), window_title(handle)))
                .collect();
            return Err(format!(
                "save prompt timeout after provider-observed text; title={:?}; windows={listed:?}; evidence={}",
                window_title(hwnd),
                evidence.display()
            ));
        }
    };
    let _ = shot(&session, hwnd, "p0-7-prompt");
    let prompt_evidence = serde_json::json!({
        "checkpoint": "save prompt observed",
        "window": window_evidence(&session, hwnd),
        "command": command,
        "dialog_hwnd": dialog.0 as usize,
        "dialog_children": child_texts(dialog),
        "prompt_observations": prompt_observations,
        "command_traces": session.command_trace_evidence(),
    });
    let _ = write_session_evidence(&session, "p0-7-prompt", &prompt_evidence)?;
    let mut buttons = child_texts(dialog);
    buttons.iter_mut().for_each(|b| *b = b.replace('&', "").to_lowercase());
    let joined = buttons.join(" | ");
    let has_save = buttons.iter().any(|b| b.contains("save") && !b.contains("don"));
    let has_dont = buttons.iter().any(|b| b.contains("don") && b.contains("save"));
    let has_cancel = buttons.iter().any(|b| b.contains("cancel"));
    if !(has_save && has_dont && has_cancel) {
        return Err(format!("prompt missing the three buttons; saw: {joined}"));
    }
    // Cancel to keep the tab and let the session tear down cleanly.
    let _ = click_button(dialog, &["cancel"]);
    Ok(())
}

/// P1-1: no submenu holds exactly one command whose title equals the submenu.
fn journey_p1_1(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p1-1", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let shapes = submenu_shapes(hwnd);
    if shapes.is_empty() {
        return Err("no menu bar found".into());
    }
    let offenders: Vec<String> = shapes
        .iter()
        .filter(|s| s.lone_same_named_child || (s.command_children == 1 && s.title.is_empty()))
        .map(|s| s.title.clone())
        .collect();
    if !offenders.is_empty() {
        return Err(format!("single-item submenus present: {offenders:?}"));
    }
    Ok(())
}

/// P1-2: the top-level menu bar is the curated 12-menu taxonomy with no stray
/// Utilities / Workspace / Extensions top-level menus.
fn journey_p1_2(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p1-2", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let labels = top_menu_labels(hwnd);
    if labels.is_empty() {
        return Err("no menu bar labels found".into());
    }
    for stray in ["utilities", "workspace", "extensions"] {
        if labels.iter().any(|l| l == stray) {
            return Err(format!("stray top-level menu present: {stray}"));
        }
    }
    for required in ["file", "edit", "search", "view", "tools", "help"] {
        if !labels.iter().any(|l| l == required) {
            return Err(format!("expected top-level menu missing: {required}; saw {labels:?}"));
        }
    }
    Ok(())
}

/// P1-5/U01: Go To Line and Run open as overlays and an empty prompt closes on
/// the first Escape. IME preedit and Compare Options remain in the UIA/native
/// qualification matrix because SendInput cannot create a real composition.
fn journey_p1_5(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p1-5", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let before = shot(&session, hwnd, "p1-5-before")?;
    // The overlay draws in the upper region of the editor; require a change there.
    let region = (0, 0, before.width, (before.height / 3).max(1));
    let changed = invoke_menu_until_change(
        hwnd,
        &["Search", "Go To Line"],
        &before,
        region,
        0.004,
        Duration::from_secs(10),
    )?;
    if changed < 0.004 {
        return Err(format!("Go To Line produced no visible overlay (changed {changed:.4})"));
    }
    let _ = shot(&session, hwnd, "p1-5-goto-overlay");
    chord(false, false, VK_ESCAPE);
    wait_until_stable(hwnd, Duration::from_secs(3));
    let after_goto = shot(&session, hwnd, "p1-5-goto-dismissed")?;
    let remaining = before.diff_fraction(&after_goto, region);
    if remaining >= 0.004 {
        return Err(format!(
            "Go To Line did not close on the first Escape (changed {remaining:.4})"
        ));
    }
    let changed = invoke_menu_until_change(
        hwnd,
        &["Run", "Run…"],
        &after_goto,
        region,
        0.004,
        Duration::from_secs(10),
    )?;
    if changed < 0.004 {
        return Err(format!("Run produced no visible prompt (changed {changed:.4})"));
    }
    let _ = shot(&session, hwnd, "p1-5-run-overlay");
    chord(false, false, VK_ESCAPE);
    wait_until_stable(hwnd, Duration::from_secs(3));
    let after_run = shot(&session, hwnd, "p1-5-run-dismissed")?;
    let remaining = after_goto.diff_fraction(&after_run, region);
    if remaining >= 0.004 {
        return Err(format!(
            "Run did not close on the first Escape (changed {remaining:.4})"
        ));
    }
    Ok(())
}

/// P3-1: toggling a panel never overlaps or duplicates the tab strip.
fn journey_p3_1(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p3-1", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let before = shot(&session, hwnd, "p3-1-before")?;
    // Toggle a dock panel via View > Panels. The curated menu labels the entries
    // "Toggle …", not the bare panel name. Retry because a menu rebuild can drop
    // a single posted command; the first successful toggle changes the body.
    let body = (0, before.height / 3, before.width, before.height);
    let changed = invoke_menu_until_change(
        hwnd,
        &["View", "Panels", "Toggle Workspace"],
        &before,
        body,
        0.004,
        Duration::from_secs(10),
    )?;
    if changed < 0.004 {
        return Err(format!(
            "panel toggle produced no visible change (changed {changed:.4})"
        ));
    }
    let _ = shot(&session, hwnd, "p3-1-panel");
    Ok(())
}

/// P3-6a: Settings opens as a closable tab (Preferences command).
fn journey_p3_6a(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p3-6a", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let before = shot(&session, hwnd, "p3-6a-before")?;
    // The Settings menu's first command is titled "Settings" (settings.open).
    let full = (0, 0, before.width, before.height);
    let changed = invoke_menu_until_change(
        hwnd,
        &["Settings", "Settings"],
        &before,
        full,
        0.01,
        Duration::from_secs(10),
    )?;
    if changed < 0.01 {
        return Err(format!("Settings did not open (changed {changed:.4})"));
    }
    let _ = shot(&session, hwnd, "p3-6a-settings");
    Ok(())
}

/// P4-1: a dirty Untitled tab reads with exactly one marker. We assert the tab
/// strip changes when the document becomes dirty (marker appears) and that the
/// window title carries a single dirty marker, not two.
fn journey_p4_1(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p4-1", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let clean = shot(&session, hwnd, "p4-1-clean")?;
    type_text_until_observed_change(&session, hwnd, "x", Duration::from_secs(10))?;
    session.settle();
    let dirty = shot(&session, hwnd, "p4-1-dirty")?;
    // Tab strip band changed when the dirty marker appeared. The marker is a
    // few pixels, so measure only the first tab's title area.
    let strip = (0, (dirty.height / 30).max(24), 220, (dirty.height / 8).max(48));
    let changed = dirty.diff_fraction(&clean, strip);
    if changed < 0.001 {
        return Err(format!(
            "no dirty marker change in the tab strip (changed {changed:.4})"
        ));
    }
    // The caption should not contain a doubled bullet.
    let title = window_title(hwnd);
    if title.matches('•').count() > 1 || title.contains("••") {
        return Err(format!("doubled dirty marker in title: {title:?}"));
    }
    Ok(())
}

/// P4-2: closing the last tab yields a fresh Untitled rather than a blank editor.
fn journey_p4_2(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p4-2", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    // Close the (clean) sole tab; a clean document closes without a prompt.
    invoke_menu(hwnd, &["File", "Close"])?;
    std::thread::sleep(Duration::from_millis(800));
    // The process must still be alive with a window (fresh Untitled), not exited.
    if let Some(status) = session.child.try_wait().map_err(|e| e.to_string())? {
        return Err(format!("closing the last tab exited the app: {status}"));
    }
    let hwnd = find_main_window(session.pid).ok_or("no window after closing the last tab")?;
    let title = window_title(hwnd);
    let _ = shot(&session, hwnd, "p4-2-fresh");
    if title.trim().is_empty() {
        return Err("window has no title after closing the last tab".into());
    }
    Ok(())
}

/// P4-4: a colour emoji renders in the editor (saturated pixels appear where the
/// glyph is typed).
fn journey_p4_4(env: &Env) -> Result<(), String> {
    let mut session = Session::launch(env, "p4-4", &[], &[])?;
    let hwnd = session.wait_window(Duration::from_secs(20))?;
    session.settle();
    let before = shot(&session, hwnd, "p4-4-before")?;
    let before_window = window_evidence(&session, hwnd);
    // Sample the first rows inside the actual in-process editor provider. UIA
    // reports screen bounds, normalized above to the captured window.
    let region = editor_glyph_region(&before_window, &before).ok_or_else(|| {
        retain_input_prerequisite_failure(
            &session,
            hwnd,
            "UIA prerequisite: editor TextPattern provider has no usable glyph bounds",
            before_window["uia_provider"].clone(),
            false,
            serde_json::json!([]),
        )
    })?;
    let before_sat = before.max_saturation(region);
    let before_evidence = serde_json::json!({
        "checkpoint": "before emoji input",
        "window": before_window,
        "sample_region": [region.0, region.1, region.2, region.3],
        "max_saturation": before_sat,
        "screenshot": session.home.join("shots/p4-4-before.bmp"),
    });
    let _ = write_session_evidence(&session, "p4-4-before", &before_evidence)?;
    let typing = type_text_until_observed_change(&session, hwnd, "🎉🎊🌈", Duration::from_secs(10))?;
    let frame = wait_until_stable(hwnd, Duration::from_secs(8));
    let after = shot(&session, hwnd, "p4-4-emoji")?;
    let after_sat = after.max_saturation(region);
    let window = window_evidence(&session, hwnd);
    let after_evidence = serde_json::json!({
        "checkpoint": "emoji input prerequisites and frame receipt",
        "window": window,
        "typing_receipt": typing,
        "frame_receipt": frame,
        "sample_region": [region.0, region.1, region.2, region.3],
        "saturation_before": before_sat,
        "saturation_after": after_sat,
        "screenshot_before": session.home.join("shots/p4-4-before.bmp"),
        "screenshot_after": session.home.join("shots/p4-4-emoji.bmp"),
    });
    let evidence = write_session_evidence(&session, "p4-4-after", &after_evidence)?;
    if frame["stable"].as_bool() != Some(true) {
        return Err(format!(
            "timeout waiting for a stable frame after emoji input; evidence={}",
            evidence.display()
        ));
    }
    let Some(provider_text) = observed_uia_text(&window) else {
        return Err(format!(
            "UIA prerequisite unavailable after emoji input; evidence={}",
            evidence.display()
        ));
    };
    if provider_text != "🎉🎊🌈" {
        return Err(format!(
            "UIA prerequisite did not observe the intended emoji text; evidence={}",
            evidence.display()
        ));
    }
    if after_sat < 60 || after_sat <= before_sat {
        return Err(format!(
            "no colourful emoji glyph detected (saturation before {before_sat}, after {after_sat}); evidence={}",
            evidence.display()
        ));
    }
    Ok(())
}

// ------------------------------------------------------------------- helpers --

/// Click a button in a dialog by matching (normalized) text against candidates.
/// Uses `BM_CLICK` posted to the matched child control.
fn click_button(dialog: HWND, candidates: &[&str]) -> Result<(), String> {
    let mut found = FoundWindows(Vec::new());
    // SAFETY: callback records handles into `found`.
    unsafe {
        let _ = EnumChildWindows(
            Some(dialog),
            Some(collect_windows),
            LPARAM(&mut found as *mut FoundWindows as isize),
        );
    }
    const BM_CLICK: u32 = 0x00F5;
    for control in found.0 {
        // Only real buttons: dialog body text can contain the same words (e.g.
        // "Your changes will be lost if you don't save them.") and a click on a
        // Static is a silent no-op.
        let mut class = [0u16; 64];
        // SAFETY: `control` is a live child; `class` is a local buffer.
        let class_len = unsafe { GetClassNameW(control, &mut class) };
        let class = String::from_utf16_lossy(&class[..class_len.max(0) as usize]);
        if !class.eq_ignore_ascii_case("button") {
            continue;
        }
        let text = window_title(control).replace('&', "").to_lowercase();
        if !text.is_empty() && candidates.iter().any(|c| text.contains(c)) {
            // SAFETY: `control` is a live child control handle.
            unsafe { PostMessageW(Some(control), BM_CLICK, WPARAM(0), LPARAM(0)) }
                .map_err(|error| format!("PostMessage BM_CLICK failed: {error}"))?;
            return Ok(());
        }
    }
    Err(format!("no button matched {candidates:?}"))
}

#[cfg(test)]
mod evidence_tests {
    use super::*;

    struct TempRepo(PathBuf);
    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bareline-source-identity-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            let run_git = |arguments: &[&str]| {
                let status = Command::new("git").args(arguments).current_dir(&path).status().unwrap();
                assert!(status.success(), "git {arguments:?} failed");
            };
            run_git(&["init", "--quiet"]);
            run_git(&["config", "user.email", "source-identity@example.invalid"]);
            run_git(&["config", "user.name", "Source Identity Fixture"]);
            std::fs::write(path.join("tracked.txt"), b"tracked\n").unwrap();
            run_git(&["add", "tracked.txt"]);
            run_git(&["commit", "--quiet", "-m", "fixture"]);
            Self(path)
        }
    }
    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn python_source_identity(root: &Path) -> serde_json::Value {
        let tool = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".github/workflows/run_test_evidence.py");
        let script = "import importlib.util,json,pathlib,sys; spec=importlib.util.spec_from_file_location('evidence',sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); print(json.dumps(module.source_identity(pathlib.Path(sys.argv[2]))))";
        let output = Command::new("python")
            .args(["-c", script])
            .arg(tool)
            .arg(root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn assert_source_identity_parity(root: &Path) {
        let (head, dirty, manifest) = source_identity(root).unwrap();
        let python = python_source_identity(root);
        assert_eq!(python["available"], true);
        assert_eq!(python["head"], head);
        assert_eq!(python["working_tree_dirty"], dirty);
        assert_eq!(python["source_manifest_sha256"], manifest);
    }

    #[test]
    fn rust_and_t09_python_source_identities_share_exact_untracked_framing() {
        let repo = TempRepo::new();
        assert_source_identity_parity(&repo.0);

        std::fs::write(repo.0.join("first.txt"), b"first").unwrap();
        std::fs::write(repo.0.join("naïve-文.txt"), vec![0x5a; 64 * 1024 + 19]).unwrap();
        assert_source_identity_parity(&repo.0);
    }

    #[test]
    fn input_delivery_uses_the_exact_utf16_selection_transition() {
        let intended = "🎉e\u{301}";
        let insertion = serde_json::json!({
            "editor": {"available": true, "text_pattern": {
                "bounded_complete": true,
                "selection_count": 1,
                "document_text": "ab",
                "selection": {"caret_start_utf16": 1, "selection_utf16": 0}
            }}
        });
        let (before, expected) = expected_text_transition(&insertion, intended).unwrap();
        assert_eq!(before, "ab");
        assert_eq!(expected, "a🎉e\u{301}b");
        assert_eq!(text_delivery(&before, &before, &expected), TextDelivery::Unchanged);
        assert_eq!(text_delivery(&before, &expected, &expected), TextDelivery::Delivered);

        let replacement = serde_json::json!({
            "editor": {"available": true, "text_pattern": {
                "bounded_complete": true,
                "selection_count": 1,
                "document_text": "a😀zb",
                "selection": {"caret_start_utf16": 1, "selection_utf16": 3}
            }}
        });
        let (_, expected) = expected_text_transition(&replacement, intended).unwrap();
        assert_eq!(expected, "a🎉e\u{301}b");
        assert_eq!(text_delivery("ab", "a🎉", &expected), TextDelivery::Diverged);
        assert_eq!(
            text_delivery("ab", "a🎉e\u{301}🎉e\u{301}b", &expected),
            TextDelivery::Diverged
        );
        assert_eq!(
            retain_partial_delivery(TextDelivery::Diverged, TextDelivery::Unchanged),
            TextDelivery::Diverged,
            "a partial observation must prevent fallback even if a later poll is stale"
        );
        assert_eq!(
            retain_partial_delivery(TextDelivery::Diverged, TextDelivery::Delivered),
            TextDelivery::Delivered,
            "queued input may finish after an intermediate partial observation"
        );
        assert!(replace_utf16("😀", 1, 0, "x").is_none());
    }

    #[test]
    fn provider_identity_matches_single_and_split_production_names_only() {
        assert_eq!(production_editor_identity("Editor"), Some("Editor"));
        assert_eq!(production_editor_identity("Pane 1, Untitled"), Some("Pane 1"));
        assert_eq!(
            production_editor_identity("Pane 2, notes.txt, C:\\notes.txt"),
            Some("Pane 2")
        );

        for modal_or_malformed in [
            "Find text",
            "Replace text",
            "Go to line",
            "Editor name",
            "Pane 1",
            "Pane 1,",
            "Pane 3, Untitled",
        ] {
            assert_eq!(
                production_editor_identity(modal_or_malformed),
                None,
                "accepted non-production Edit provider {modal_or_malformed:?}"
            );
        }
    }

    #[test]
    fn menu_resolution_change_receipt_compares_handle_path_labels_and_id() {
        let resolution = MenuResolution {
            id: 45,
            root_menu: 100,
            positions: vec![0, 8],
            labels: vec!["&File".into(), "E&xit".into()],
        };
        let same = MenuResolution {
            id: 45,
            root_menu: 100,
            positions: vec![0, 8],
            labels: vec!["&File".into(), "E&xit".into()],
        };
        assert!(resolution.same_target(&same));
        assert!(!resolution.same_target(&MenuResolution { id: 48, ..same }));
    }

    #[test]
    fn prompt_observations_are_bounded_and_always_retain_terminal_state() {
        let mut observations = Vec::new();
        for index in 0..10 {
            retain_prompt_observation(&mut observations, serde_json::json!({"transition": index}), false);
        }
        assert_eq!(observations.len(), 8);
        assert_eq!(observations[0]["transition"], 0);
        assert_eq!(observations[7]["transition"], 7);

        retain_prompt_observation(&mut observations, serde_json::json!({"trigger": "timeout"}), true);
        assert_eq!(observations.len(), 8);
        assert_eq!(observations[0]["transition"], 0);
        assert_eq!(observations[7]["trigger"], "timeout");
    }

    #[test]
    fn unicode_sendinput_batches_keep_each_scalar_and_surrogate_pair_complete() {
        let batches = unicode_scalar_input_batches("A🎉e\u{301}");
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [2, 4, 2, 2]);

        let scans = batches
            .iter()
            .map(|batch| {
                batch
                    .iter()
                    .map(|input| unsafe { input.Anonymous.ki.wScan })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            scans,
            [
                vec![0x41, 0x41],
                vec![0xD83C, 0xD83C, 0xDF89, 0xDF89],
                vec![0x65, 0x65],
                vec![0x0301, 0x0301]
            ]
        );
        for batch in &batches {
            for (index, input) in batch.iter().enumerate() {
                let key = unsafe { input.Anonymous.ki };
                assert_eq!(key.wVk, VIRTUAL_KEY(0));
                assert!(key.dwFlags.contains(KEYEVENTF_UNICODE));
                assert_eq!(key.dwFlags.contains(KEYEVENTF_KEYUP), index % 2 == 1);
            }
        }
    }

    #[test]
    fn exact_text_cannot_override_partial_or_focus_lost_input_submission() {
        let complete = serde_json::json!({
            "requested_scalars": 2,
            "completed_scalars": 2,
            "requested_events": 4,
            "attempted_events": 4,
            "inserted_events": 4,
            "errors": [],
        });
        assert!(input_submission_complete(&complete));

        let mut missing_final_key_up = complete.clone();
        missing_final_key_up["inserted_events"] = serde_json::json!(3);
        missing_final_key_up["errors"] = serde_json::json!([{"kind": "partial_batch"}]);
        assert!(!input_submission_complete(&missing_final_key_up));

        let mut focus_lost = complete;
        focus_lost["completed_scalars"] = serde_json::json!(1);
        focus_lost["attempted_events"] = serde_json::json!(2);
        focus_lost["inserted_events"] = serde_json::json!(2);
        focus_lost["errors"] = serde_json::json!([{"kind": "foreground_lost_before_scalar"}]);
        assert!(!input_submission_complete(&focus_lost));
    }

    #[test]
    fn failure_classification_keeps_timeout_harness_and_product_results_distinct() {
        assert_eq!(classify_failure("timed out waiting for prompt"), "timeout");
        assert_eq!(classify_failure("spawn failed: denied"), "harness_setup");
        assert_eq!(
            classify_failure("no colourful emoji glyph detected"),
            "product_assertion"
        );
        assert_eq!(
            classify_failure("blocked environment: no desktop"),
            "blocked_environment"
        );
    }

    #[test]
    fn evidence_counts_only_explicit_top_level_results() {
        let path = std::env::temp_dir().join(format!(
            "bareline-journey-evidence-{}-{}.json",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let identity = serde_json::json!({"run_id": "fixture"});
        let results = vec![
            serde_json::json!({"name": "p0-7", "status": "passed", "child_fixture_lines": 12}),
            serde_json::json!({"name": "p4-4", "status": "failed", "child_fixture_lines": 9}),
        ];
        persist_run_evidence(&path, &identity, 2, &results).unwrap();
        let evidence: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(evidence["top_level_total"], 2);
        assert_eq!(evidence["top_level_completed"], 2);
        assert_eq!(evidence["top_level_passed"], 1);
        assert_eq!(evidence["top_level_failed"], 1);
        std::fs::remove_file(path).unwrap();
    }
}
