// SPDX-License-Identifier: MPL-2.0
//! The ordinary journeys of tests/e2e/journeys.json on Linux and macOS.
//!
//! Each procedure follows its Windows counterpart (tests/e2e/native_*.ps1)
//! step for step, with the oracles these systems offer without an
//! accessibility tree: the exact bytes the editor saves, the tab strip's
//! pixels for the modified marker, visible changes in the editor body, files
//! in the isolated profile, the close-command trace and the process tree.
//! Text the Windows driver reads through UI Automation is recorded as an
//! unobservable check of the step, never silently dropped. Where the Windows
//! route opens a native file dialog, the file is given on the command line or
//! restored by the session (a substituted route, also recorded); a step whose
//! action has no such route asks the dialog service and is classified as
//! `service_not_wired` while the shell's seam still holds the stand-in. On
//! Linux the desktop's portal stand-in answers those choosers with the path
//! the step stages, so the editor's portal request is checked but no chooser
//! window is driven (also recorded as substituted).
//!
//! Commands that Windows posts through the native menu run through the
//! command palette (Primary+Shift+P, the exact title, Enter): Linux has no
//! native menu bar, and macOS reaches the same commands this way until the
//! menu bar is wired.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::steps::{self, Class, Failure, Gap, GapKind, Service, StepRecord, StepStatus};
use super::{Env, Image, Journey, save_bmp};

#[cfg(target_os = "linux")]
use super::linux::desktop;
#[cfg(target_os = "macos")]
use super::macos::desktop;

pub(super) fn journeys() -> Vec<Journey> {
    vec![
        Journey {
            name: "smoke",
            summary: "launch hidden and exit after first frame",
            run: journey_smoke,
        },
        Journey {
            name: "plain_text",
            summary: "Quick plain-text edit",
            run: |env| drive(env, "plain_text", plain_text),
        },
        Journey {
            name: "code_config",
            summary: "Code/config edit",
            run: |env| drive(env, "code_config", code_config),
        },
        Journey {
            name: "regex_transform",
            summary: "Regex transform",
            run: |env| drive(env, "regex_transform", regex_transform),
        },
        Journey {
            name: "column_multi_cursor",
            summary: "Column/multi-cursor transform",
            run: |env| drive(env, "column_multi_cursor", column_multi_cursor),
        },
        Journey {
            name: "huge_log_tail",
            summary: "Huge log search/tail",
            run: |env| drive(env, "huge_log_tail", huge_log_tail),
        },
        Journey {
            name: "workspace",
            summary: "Workspace navigation",
            run: |env| drive(env, "workspace", workspace),
        },
        Journey {
            name: "udl",
            summary: "UDL import",
            run: |env| drive(env, "udl", udl),
        },
        Journey {
            name: "macro_external",
            summary: "Macro/external command",
            run: |env| drive(env, "macro_external", macro_external),
        },
        Journey {
            name: "split_clone_sync",
            summary: "Split/clone/sync",
            run: |env| drive(env, "split_clone_sync", split_clone_sync),
        },
        Journey {
            name: "portable",
            summary: "Portable usage",
            run: |env| drive(env, "portable", portable),
        },
        Journey {
            name: "ui_regressions",
            summary: "Manual-QA UI regressions",
            run: |env| drive(env, "ui_regressions", ui_regressions),
        },
    ]
}

// ------------------------------------------------------------------- desktop --

/// A top-level window as the desktop tools report it, in logical pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) id: String,
    pub(super) pid: Option<u32>,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) title: Option<String>,
}

impl Window {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "pid": self.pid,
            "rect": [self.x, self.y, self.width, self.height],
            "title": self.title,
        })
    }
}

/// What the journeys need from the window system. Key chords use neutral
/// names joined by `+`: `Primary` (Ctrl on Linux, Command on macOS), `Shift`,
/// `Alt`, letters, `Return`, `Escape`, `Tab`, `Home`, `End`, `Up`, `Down`,
/// `Left`, `Right`, `PageDown`, `Space` and `F1`..`F12`.
pub(super) trait Desktop {
    /// The session and tools this desktop drives, for the evidence.
    fn describe(&self) -> serde_json::Value;
    /// Visible top-level windows owned by `pid`.
    fn windows(&self, pid: u32) -> Result<Vec<Window>, Failure>;
    /// Every visible top-level window (a portal dialog belongs to another process).
    fn all_windows(&self) -> Result<Vec<Window>, Failure>;
    fn focus(&self, window: &Window) -> Result<(), Failure>;
    fn key(&self, window: &Window, chord: &str) -> Result<(), Failure>;
    fn text(&self, window: &Window, text: &str) -> Result<(), Failure>;
    fn capture(&self, window: &Window) -> Result<Image, Failure>;
    /// Device pixels of window frame above the editor's own content.
    fn content_top(&self, window: &Window, image: &Image) -> i32;
    /// Re-encode a BMP screenshot as PNG; false keeps the BMP.
    fn png(&self, bmp: &Path, png: &Path) -> bool;
    /// `Ok(host)` when native file dialogs can appear in this session at all
    /// (Linux: an XDG desktop portal on the session bus), `Err(why)` otherwise.
    fn dialog_host(&self) -> Result<String, String>;
    /// Variables every editor on this desktop is launched with (`None`
    /// removes one): its own display and session bus.
    fn environment(&self) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
        Vec::new()
    }
    /// When the desktop answers file choosers itself (the harness's portal
    /// stand-in), stage `target` as the next chooser's answer and return how
    /// many requests it has answered so far; `None` when a chooser window
    /// must be driven instead.
    fn answer_next_chooser(&self, _target: &Path) -> Result<Option<usize>, Failure> {
        Ok(None)
    }
    /// The chooser requests the desktop answered itself, oldest first.
    fn chooser_requests(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
}

/// Run a desktop tool to completion within `deadline`, draining its pipes on a
/// thread (a capture writes megabytes to stdout). A tool that cannot start is
/// an environment failure; one that does not answer in time (a hung display,
/// a permission prompt nobody answers) is killed and reported as a timeout.
pub(super) fn output_within(command: &mut Command, name: &str, deadline: Duration) -> Result<Output, Failure> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::environment(format!("{name} is not available: {error}")))?;
    let pid = child.id();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(deadline) {
        Ok(output) => output.map_err(|error| Failure::harness(format!("{name}: {error}"))),
        Err(_) => {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .stderr(Stdio::null())
                .status();
            Err(Failure::timeout(format!(
                "{name} did not answer within {} s",
                deadline.as_secs()
            )))
        }
    }
}

/// The isolated profile layout below one scratch home.
struct Layout {
    home: PathBuf,
    /// Settings, session, recovery, macros and languages of the installed profile.
    profile: PathBuf,
    logs: PathBuf,
    /// Folders an installed Bareline may write; a portable run must leave them empty.
    installed_roots: Vec<PathBuf>,
    /// The private runtime folder (XDG_RUNTIME_DIR; on macOS also TMPDIR,
    /// where the editor keeps its runtime files). It lives under /tmp, not
    /// in the scratch home: the instance socket's path must fit a Unix socket
    /// address (108 bytes on Linux, 104 on macOS), and a socket that cannot
    /// bind makes every window independent, which never saves the session.
    runtime: PathBuf,
}

impl Layout {
    fn new(home: &Path) -> Self {
        let (profile, logs) = if cfg!(target_os = "macos") {
            (
                home.join("Library/Application Support/Bareline"),
                home.join("Library/Logs/Bareline"),
            )
        } else {
            (home.join("data/bareline"), home.join("state/bareline/logs"))
        };
        let installed_roots = vec![
            profile.clone(),
            logs.clone(),
            home.join("config/bareline"),
            home.join("cache/bareline"),
            home.join("state/bareline"),
        ];
        let digest = Sha256::digest(home.as_os_str().as_encoded_bytes());
        let key: String = digest[..6].iter().map(|byte| format!("{byte:02x}")).collect();
        Self {
            home: home.to_path_buf(),
            profile,
            logs,
            installed_roots,
            runtime: PathBuf::from(format!("/tmp/bl-journey-{key}")),
        }
    }

    fn environment(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home.clone()),
            ("XDG_DATA_HOME", self.home.join("data")),
            ("XDG_CONFIG_HOME", self.home.join("config")),
            ("XDG_STATE_HOME", self.home.join("state")),
            ("XDG_CACHE_HOME", self.home.join("cache")),
            ("XDG_RUNTIME_DIR", self.runtime.clone()),
            (
                "TMPDIR",
                if cfg!(target_os = "macos") {
                    self.runtime.clone()
                } else {
                    self.home.join("temp")
                },
            ),
        ]
    }

    fn create(&self) -> Result<(), Failure> {
        for (_, path) in self.environment() {
            std::fs::create_dir_all(&path).map_err(|error| Failure::harness(format!("profile folder: {error}")))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.runtime, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| Failure::harness(format!("runtime folder: {error}")))?;
        }
        Ok(())
    }

    /// Remove the private runtime folder once nothing of the attempt runs.
    fn remove_runtime(&self) {
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// Window regions the oracles compare, relative to the captured image.
#[derive(Clone, Copy, Debug)]
enum Region {
    /// The left half of the tab strip, where the active tab's modified marker is drawn.
    Tabs,
    /// The first tab alone (its first 140 logical pixels).
    FirstTab,
    /// The editor body without gutter, scroll bar, toasts and status bar.
    Body,
    Left,
    Right,
    /// The middle of the window, where in-app prompts appear.
    Center,
    /// The right part of the find bar, where the search status is drawn.
    FindStatus,
}

// ----------------------------------------------------------------- the engine --

/// How a step ends early: a classified failure, or a skip with its reason.
enum Stop {
    Fail(Failure),
    Skip(String),
}

impl From<Failure> for Stop {
    fn from(failure: Failure) -> Self {
        Stop::Fail(failure)
    }
}

type StepResult = Result<(), Stop>;

/// Why a pixel wait ended without the expected state.
enum Miss {
    /// The region's last difference from the reference, which the caller's
    /// oracle rejected: a product failure.
    Pixels(f64),
    /// The capture itself failed, with the desktop's own class.
    Capture(Failure),
}

impl Miss {
    fn into_failure(self, mismatch: impl FnOnce(f64) -> String) -> Failure {
        match self {
            Miss::Pixels(diff) => Failure::product(mismatch(diff)),
            Miss::Capture(failure) => failure,
        }
    }
}

struct Editor {
    child: Child,
    pid: u32,
    window: Window,
    stdout: PathBuf,
    stderr: PathBuf,
}

/// Why a launched editor showed no window.
enum NoWindow {
    /// The display refused the editor's connection: the session's failure.
    Refused,
    Failed(Failure),
}

/// Launches again after an X server refusal before the display is blamed.
const LAUNCH_RETRIES: u32 = 2;

/// Whether an editor's standard error shows its display refusing the
/// connection (the X server under Xvfb), which is never the product's failure.
fn refused_by_display(stderr: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        super::linux::x_connection_refused(stderr)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = stderr;
        false
    }
}

struct Launch<'a> {
    executable: Option<&'a Path>,
    session: bool,
    files: &'a [&'a Path],
}

impl<'a> Launch<'a> {
    fn files(files: &'a [&'a Path]) -> Self {
        Self {
            executable: None,
            session: false,
            files,
        }
    }
}

/// One attempt of one journey: its isolated home, its evidence directory and
/// the editor it owns.
struct Run<'e> {
    env: &'e Env,
    desktop: Box<dyn Desktop>,
    journey: &'static str,
    evidence: PathBuf,
    scratch: PathBuf,
    layout: Layout,
    editor: Option<Editor>,
    launches: usize,
    shots: usize,
    last_shot: Option<PathBuf>,
    records: Vec<serde_json::Value>,
    steps: Vec<StepRecord>,
    gaps: Vec<Gap>,
    blocked: Option<String>,
    started: Instant,
    fixture: serde_json::Value,
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Run one journey attempt and leave its report for the driver.
fn drive(env: &Env, journey: &'static str, procedure: fn(&mut Run) -> Result<(), Failure>) -> Result<(), String> {
    let attempt = env.attempt.get();
    let evidence = env.output.join(format!("{journey}-{attempt}"));
    if evidence.exists() {
        // Never overwrite an earlier run's evidence: each run takes its own --output.
        return Err(format!(
            "evidence directory {} already exists; pass a new --output for each run",
            evidence.display()
        ));
    }
    std::fs::create_dir_all(evidence.join("shots")).map_err(|error| error.to_string())?;
    let (scratch, _) = env.scratch(journey)?;
    let platform = platform_evidence(env, &scratch);
    let desktop = match desktop(&env.root, &scratch) {
        Ok(desktop) => desktop,
        Err(failure) => {
            // Without a desktop nothing can run: report every step.
            let steps = manifest_steps(env, journey)
                .into_iter()
                .enumerate()
                .map(|(index, id)| StepRecord {
                    id,
                    status: if index == 0 {
                        StepStatus::Fail(failure.clone())
                    } else {
                        StepStatus::NotRun("No desktop session; nothing was submitted.".into())
                    },
                    observed: String::new(),
                    gaps: Vec::new(),
                })
                .collect::<Vec<_>>();
            return finish_report(env, journey, &evidence, platform, &steps);
        }
    };
    let mut run = Run {
        env,
        desktop,
        journey,
        evidence: evidence.clone(),
        layout: Layout::new(&scratch),
        scratch,
        editor: None,
        launches: 0,
        shots: 0,
        last_shot: None,
        records: Vec::new(),
        steps: Vec::new(),
        gaps: Vec::new(),
        blocked: None,
        started: Instant::now(),
        fixture: serde_json::Value::Null,
    };
    let mut platform = platform;
    platform["desktop"] = run.desktop.describe();
    let setup = run.layout.create().and_then(|()| procedure(&mut run));
    if let Err(failure) = setup {
        // A failure outside any step (fixture generation) fails the first step.
        let first = manifest_steps(env, journey)
            .into_iter()
            .next()
            .unwrap_or_else(|| "s1".into());
        if !run.steps.iter().any(|step| step.id == first) {
            run.steps.push(StepRecord {
                id: first,
                status: StepStatus::Fail(failure),
                observed: String::new(),
                gaps: Vec::new(),
            });
        }
    }
    run.cleanup();
    // The generated logs (256 and 512 MiB) are reproducible from their recipes.
    for entry in std::fs::read_dir(&run.scratch).into_iter().flatten().flatten() {
        if entry
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 64 * 1024 * 1024)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let reported: Vec<String> = run.steps.iter().map(|step| step.id.clone()).collect();
    for id in manifest_steps(env, journey) {
        if !reported.contains(&id) {
            run.steps.push(StepRecord {
                id,
                status: StepStatus::NotRun("Prerequisite failed; dependent action was not submitted.".into()),
                observed: String::new(),
                gaps: Vec::new(),
            });
        }
    }
    platform["fixture"] = run.fixture.clone();
    let steps = std::mem::take(&mut run.steps);
    drop(run);
    finish_report(env, journey, &evidence, platform, &steps)
}

fn finish_report(
    env: &Env,
    journey: &str,
    evidence: &Path,
    platform: serde_json::Value,
    steps: &[StepRecord],
) -> Result<(), String> {
    let report = steps::report(journey, env.attempt.get(), evidence, platform, steps);
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    std::fs::write(evidence.join("result.json"), bytes).map_err(|error| error.to_string())?;
    let failed = report["outcome"] == "failed";
    let line = steps::outcome_line(&report);
    *env.report.borrow_mut() = Some(report);
    if failed { Err(line) } else { Ok(()) }
}

/// The step ids of `journey` in tests/e2e/journeys.json.
fn manifest_steps(env: &Env, journey: &str) -> Vec<String> {
    let manifest = std::fs::read(env.root.join("tests/e2e/journeys.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    manifest
        .as_ref()
        .and_then(|value| value["journeys"].as_array())
        .and_then(|rows| rows.iter().find(|row| row["id"] == journey))
        .and_then(|row| row["steps"].as_array())
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn platform_evidence(env: &Env, scratch: &Path) -> serde_json::Value {
    let services: serde_json::Map<String, serde_json::Value> = Service::ALL
        .iter()
        .map(|service| {
            let state = match steps::seam_stand_in(&env.root, *service) {
                Some(true) => "stand-in",
                Some(false) => "wired",
                None => "unknown",
            };
            (service.name().to_owned(), serde_json::json!(state))
        })
        .collect();
    serde_json::json!({
        "run_id": env.run_id,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "executable": env.exe,
        "scratch": scratch,
        "seam_services": services,
        "display": std::env::var("DISPLAY").ok(),
        "wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
    })
}

impl Run<'_> {
    // ------------------------------------------------------------ steps ---

    /// A step that depends on the previous ones: after a failure or a skip it
    /// is reported as not run.
    fn step(&mut self, id: &str, observed: &str, body: impl FnOnce(&mut Self) -> StepResult) {
        if let Some(reason) = self.blocked.clone() {
            self.steps.push(StepRecord {
                id: id.into(),
                status: StepStatus::NotRun(reason),
                observed: String::new(),
                gaps: Vec::new(),
            });
            return;
        }
        self.run_step(id, observed, body, true);
    }

    /// A step that starts from its own fresh editor, so an earlier failure does
    /// not hide it (the regression procedure's steps).
    fn independent_step(&mut self, id: &str, observed: &str, body: impl FnOnce(&mut Self) -> StepResult) {
        self.run_step(id, observed, body, false);
    }

    fn run_step(&mut self, id: &str, observed: &str, body: impl FnOnce(&mut Self) -> StepResult, dependent: bool) {
        self.gaps.clear();
        self.record(&format!("{id} start"), serde_json::json!({}));
        let result = body(self);
        let gaps = std::mem::take(&mut self.gaps);
        let status = match result {
            Ok(()) => StepStatus::Pass,
            Err(Stop::Fail(failure)) => {
                let screenshot = self
                    .shot(&format!("{id} failed"))
                    .ok()
                    .and_then(|_| self.last_shot.clone());
                self.record(
                    &format!("{id} failure"),
                    serde_json::json!({"class": failure.class.name(), "detail": failure.detail, "screenshot": screenshot}),
                );
                if dependent {
                    self.blocked = Some("Prerequisite step failed; no dependent action submitted.".into());
                }
                StepStatus::Fail(failure)
            }
            Err(Stop::Skip(reason)) => {
                self.record(&format!("{id} skipped"), serde_json::json!({"reason": reason}));
                if dependent {
                    self.blocked = Some(format!(
                        "Prerequisite step {id} was skipped; no dependent action submitted."
                    ));
                }
                StepStatus::Skipped(reason)
            }
        };
        self.steps.push(StepRecord {
            id: id.into(),
            status,
            observed: observed.into(),
            gaps,
        });
    }

    fn gap(&mut self, check: &str, kind: GapKind, note: &str) {
        self.gaps.push(Gap {
            check: check.into(),
            kind,
            note: note.into(),
        });
    }

    /// A check the Windows driver makes through UI Automation.
    fn unobservable(&mut self, check: &str) {
        self.gap(
            check,
            GapKind::Unobservable(Service::Accessibility),
            "read through UI Automation on Windows; this platform publishes no accessibility tree yet",
        );
    }

    /// The Windows route opens a native dialog; this run used `route` instead.
    fn substituted(&mut self, check: &str, route: &str) {
        self.gap(check, GapKind::Substituted, route);
    }

    // ----------------------------------------------------------- evidence ---

    fn record(&mut self, stage: &str, details: serde_json::Value) {
        self.records.push(serde_json::json!({
            "stage": stage,
            "elapsed_ms": self.started.elapsed().as_millis(),
            "details": details,
        }));
        // Persist before the next action, so a stalled run keeps its checkpoints.
        let document = serde_json::json!({"journey": self.journey, "records": self.records});
        if let Ok(bytes) = serde_json::to_vec_pretty(&document) {
            let _ = std::fs::write(self.evidence.join("observations.json"), bytes);
        }
    }

    /// Keep a small file from the scratch as evidence.
    fn retain(&mut self, path: &Path, name: &str) {
        if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= 1024 * 1024) {
            let _ = std::fs::copy(path, self.evidence.join(name));
        }
    }

    // ------------------------------------------------------------ fixtures ---

    /// Evaluate one expression over the Python fixture modules beside the
    /// Windows driver, so both platforms share the same independent oracles.
    fn python(&self, expression: &str) -> Result<serde_json::Value, Failure> {
        let python = std::env::var_os("BARELINE_QA_PYTHON").unwrap_or_else(|| "python3".into());
        let script = format!(
            "import json, sys\nsys.path.insert(0, sys.argv[1])\nprint(json.dumps({expression}, ensure_ascii=False))"
        );
        // Generating the 512 MiB log is the slowest evaluation; a wedged
        // interpreter still ends the step long before the job's timeout.
        let output = output_within(
            Command::new(python)
                .args(["-c", &script])
                .arg(self.env.root.join("tests/e2e"))
                .env("PYTHONDONTWRITEBYTECODE", "1"),
            "python fixture",
            Duration::from_secs(300),
        )
        .map_err(|failure| Failure::harness(format!("python fixture unavailable: {}", failure.detail)))?;
        if !output.status.success() {
            return Err(Failure::harness(format!(
                "fixture {expression} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| Failure::harness(format!("fixture JSON: {error}")))
    }

    fn fixture_text(fixture: &serde_json::Value, key: &str) -> Result<String, Failure> {
        fixture[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Failure::harness(format!("fixture field {key} missing")))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| Failure::harness(error.to_string()))?;
        }
        std::fs::write(path, bytes).map_err(|error| Failure::harness(format!("{}: {error}", path.display())))
    }

    /// The settings the Windows driver writes before its launch.
    fn write_settings(&self, profile: &Path, encoding: &str, eol: &str) -> Result<PathBuf, Failure> {
        let path = profile.join("settings.toml");
        let text = format!(
            "schema_version=1\n[theme]\nmode='dark'\n[files]\ndefault_encoding='{encoding}'\ndefault_eol='{eol}'\nautosave_seconds=0\n"
        );
        self.write(&path, text.as_bytes())?;
        Ok(path)
    }

    // -------------------------------------------------------------- editor ---

    fn launch(&mut self, launch: Launch) -> Result<(), Failure> {
        if self.editor.is_some() {
            return Err(Failure::harness("Previous owned editor is still running"));
        }
        self.launches += 1;
        let number = self.launches;
        let executable = launch.executable.unwrap_or(&self.env.exe).to_path_buf();
        let mut arguments: Vec<std::ffi::OsString> = vec!["--software".into()];
        if launch.session {
            // Either flag makes the window independent, and an independent
            // window neither restores nor saves the session (shell/instance.rs
            // `prepare`, the same code on Windows). The attempt's profile and
            // runtime folder are its own, so no other window can take this
            // launch: it becomes the profile's owner and keeps the session.
            self.substituted(
                "--new-instance and --no-extensions of the Windows launch",
                SESSION_ROUTE,
            );
        } else {
            arguments.extend(["--no-session".into(), "--no-extensions".into(), "--new-instance".into()]);
        }
        arguments.extend(launch.files.iter().map(|path| path.as_os_str().to_owned()));
        let stdout = self.evidence.join(format!("editor-{number}.stdout.log"));
        let stderr = self.evidence.join(format!("editor-{number}.stderr.log"));
        let trace = self.evidence.join(format!("command-trace-{number}.jsonl"));
        let open = |path: &Path| {
            std::fs::File::create(path).map_err(|error| Failure::harness(format!("{}: {error}", path.display())))
        };
        let mut refusals: u32 = 0;
        let (mut editor, window, frame) = loop {
            let mut command = Command::new(&executable);
            command
                .args(&arguments)
                .current_dir(&self.scratch)
                .env("BARELINE_QA_COMMAND_TRACE", &trace)
                .stdin(Stdio::null())
                .stdout(open(&stdout)?)
                .stderr(open(&stderr)?);
            for (name, value) in self.layout.environment() {
                command.env(name, value);
            }
            if let Some(bus) = session_bus() {
                command.env("DBUS_SESSION_BUS_ADDRESS", bus);
            }
            // The desktop's own display and bus come last and win.
            for (name, value) in self.desktop.environment() {
                match value {
                    Some(value) => command.env(name, value),
                    None => command.env_remove(name),
                };
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // Its own process group, so cleanup reaches what the editor started.
                command.process_group(0);
            }
            let child = command
                .spawn()
                .map_err(|error| Failure::harness(format!("spawn failed: {error}")))?;
            let pid = child.id();
            let mut editor = Editor {
                child,
                pid,
                window: Window {
                    id: String::new(),
                    pid: Some(pid),
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                    title: None,
                },
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            };
            match self.await_window(&mut editor) {
                Ok((window, frame)) => break (editor, window, frame),
                Err(NoWindow::Failed(failure)) => {
                    self.editor = Some(editor);
                    return Err(failure);
                }
                // The display refused the connection before the editor did
                // anything: launch again after a pause, keeping the refused
                // attempt's output, and blame the display if it persists.
                Err(NoWindow::Refused) => {
                    refusals += 1;
                    let kept = |path: &Path| path.with_extension(format!("refused-{refusals}.log"));
                    let _ = std::fs::rename(&stderr, kept(&stderr));
                    let _ = std::fs::rename(&stdout, kept(&stdout));
                    self.record(
                        &format!("owned launch {number} refused by the X server"),
                        serde_json::json!({"refusal": refusals, "pid": pid, "stderr": kept(&stderr)}),
                    );
                    if refusals > LAUNCH_RETRIES {
                        return Err(Failure::environment(format!(
                            "Editor exited before showing a window: the X server refused the connection {refusals} times"
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(500 << refusals));
                }
            }
        };
        let pid = editor.pid;
        editor.window = window.clone();
        self.editor = Some(editor);
        self.desktop.focus(&window)?;
        std::thread::sleep(Duration::from_millis(600));
        // The first-frame event can precede the first presented pixels: a
        // reference captured from a still black window makes every later
        // pixel check meaningless, so wait until the window shows its content.
        let painted_after = Instant::now();
        let deadline = painted_after + Duration::from_secs(10);
        while !painted(&self.desktop.capture(&window)?) {
            if Instant::now() >= deadline {
                return Err(Failure::timeout(
                    "Owned editor window was not ready before the startup deadline: it stayed one colour",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.record(
            &format!("owned launch {number}"),
            serde_json::json!({
                "pid": pid,
                "executable": executable,
                "arguments": arguments.iter().map(|value| value.to_string_lossy()).collect::<Vec<_>>(),
                "window": window.json(),
                "first_frame": frame,
                "painted_wait_ms": painted_after.elapsed().as_millis(),
            }),
        );
        Ok(())
    }

    /// Wait until the launched editor shows a window wider than 100 pixels and
    /// reports its first frame.
    fn await_window(&mut self, editor: &mut Editor) -> Result<(Window, serde_json::Value), NoWindow> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(Some(status)) = editor.child.try_wait() {
                let errors = std::fs::read_to_string(&editor.stderr).unwrap_or_default();
                if refused_by_display(&errors) {
                    return Err(NoWindow::Refused);
                }
                return Err(NoWindow::Failed(Failure::product(format!(
                    "Editor exited before showing a window: {status}"
                ))));
            }
            let windows = self.desktop.windows(editor.pid).map_err(NoWindow::Failed)?;
            let frame = first_frame_event(&editor.stdout);
            if let (Some(window), Some(frame)) = (windows.into_iter().max_by_key(|w| w.width * w.height), frame)
                && window.width > 100
            {
                return Ok((window, frame));
            }
            if Instant::now() >= deadline {
                return Err(NoWindow::Failed(Failure::timeout(
                    "Owned editor window and first frame were not ready before the startup deadline",
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn editor(&mut self) -> Result<&mut Editor, Failure> {
        let editor = self
            .editor
            .as_mut()
            .ok_or_else(|| Failure::harness("No owned editor is running"))?;
        if let Ok(Some(status)) = editor.child.try_wait() {
            return Err(Failure::product(format!("The editor exited unexpectedly: {status}")));
        }
        Ok(editor)
    }

    fn window(&mut self) -> Result<Window, Failure> {
        Ok(self.editor()?.window.clone())
    }

    fn pid(&mut self) -> Result<u32, Failure> {
        Ok(self.editor()?.pid)
    }

    /// Run the Exit command and require a clean exit.
    fn exit(&mut self) -> Result<(), Failure> {
        self.command("Exit")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let editor = self
            .editor
            .as_mut()
            .ok_or_else(|| Failure::harness("No owned editor"))?;
        let status = loop {
            if let Ok(Some(status)) = editor.child.try_wait() {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(Failure::product("Clean editor Exit timed out"));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let pid = editor.pid;
        self.editor = None;
        self.record(
            &format!("owned exit {}", self.launches),
            serde_json::json!({"pid": pid, "status": status.to_string(), "code": status.code()}),
        );
        if status.success() {
            Ok(())
        } else {
            Err(Failure::product(format!(
                "Editor exited with a failure status: {status}"
            )))
        }
    }

    /// Kill the owned editor and its process group.
    fn kill(&mut self) {
        if let Some(mut editor) = self.editor.take() {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", editor.pid)])
                .stderr(Stdio::null())
                .status();
            let _ = editor.child.kill();
            let _ = editor.child.wait();
        }
    }

    /// End of the attempt: a clean Exit when every step passed, otherwise kill.
    fn cleanup(&mut self) {
        let clean = self
            .steps
            .iter()
            .all(|step| !matches!(step.status, StepStatus::Fail(_)));
        if clean
            && self.editor.is_some()
            && let Err(failure) = self.exit()
        {
            self.record("cleanup failure", serde_json::json!({"detail": failure.detail}));
        }
        self.kill();
        self.layout.remove_runtime();
        // The diagnostics log of the installed profile (a portable run keeps its own).
        let log = self.layout.logs.join("bareline.log");
        self.retain(&log, "bareline.log");
    }

    // --------------------------------------------------------------- input ---

    fn key(&mut self, chord: &str) -> Result<(), Failure> {
        let window = self.window()?;
        self.desktop.key(&window, chord)?;
        std::thread::sleep(Duration::from_millis(80));
        Ok(())
    }

    fn keys(&mut self, chords: &[&str]) -> Result<(), Failure> {
        chords.iter().try_for_each(|chord| self.key(chord))
    }

    fn text(&mut self, text: &str) -> Result<(), Failure> {
        let window = self.window()?;
        self.desktop.text(&window, text)?;
        std::thread::sleep(Duration::from_millis(120));
        Ok(())
    }

    /// Run a command by its exact title through the command palette.
    fn command(&mut self, title: &str) -> Result<(), Failure> {
        self.key("Primary+Shift+P")?;
        std::thread::sleep(Duration::from_millis(300));
        self.text(title)?;
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.shot(&format!("palette {title}"));
        self.key("Return")?;
        std::thread::sleep(Duration::from_millis(500));
        self.record("palette command", serde_json::json!({"title": title}));
        Ok(())
    }

    // --------------------------------------------------------------- pixels ---

    fn capture(&mut self) -> Result<Image, Failure> {
        let window = self.window()?;
        self.desktop.capture(&window)
    }

    /// Capture and keep a numbered screenshot.
    fn shot(&mut self, label: &str) -> Result<Image, Failure> {
        let image = self.capture()?;
        self.shots += 1;
        let slug: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let base = self.evidence.join("shots").join(format!("{:02}-{slug}", self.shots));
        let bmp = base.with_extension("bmp");
        let png = base.with_extension("png");
        if save_bmp(&image, &bmp).is_ok() {
            if self.desktop.png(&bmp, &png) {
                let _ = std::fs::remove_file(&bmp);
                self.last_shot = Some(png);
            } else {
                self.last_shot = Some(bmp);
            }
        }
        Ok(image)
    }

    fn rect(&self, region: Region, image: &Image) -> (i32, i32, i32, i32) {
        let window = self.editor.as_ref().map(|editor| editor.window.clone());
        let scale = window
            .as_ref()
            .filter(|window| window.width > 0)
            .map_or(1, |window| (image.width / window.width).max(1));
        let top = window
            .as_ref()
            .map_or(0, |window| self.desktop.content_top(window, image));
        let (w, h) = (image.width, image.height);
        let bottom = top + (h - top) * 3 / 4;
        match region {
            Region::Tabs => (0, top, w / 2, top + 33 * scale),
            Region::FirstTab => (0, top, (w / 2).min(140 * scale), top + 33 * scale),
            Region::Body => (56 * scale, top + 36 * scale, w - 24 * scale, bottom),
            Region::Left => (56 * scale, top + 36 * scale, w / 2 - 8 * scale, bottom),
            Region::Right => (w / 2 + 8 * scale, top + 36 * scale, w - 24 * scale, bottom),
            Region::Center => (w / 4, h / 4, w * 3 / 4, h * 3 / 4),
            Region::FindStatus => (w * 3 / 5, top + 40 * scale, w, top + 80 * scale),
        }
    }

    /// Wait until `region` differs from `reference` by at least `threshold`.
    fn expect_change(
        &mut self,
        reference: &Image,
        region: Region,
        threshold: f64,
        stage: &str,
    ) -> Result<Image, Failure> {
        self.expect_change_within(reference, region, threshold, stage, Duration::from_secs(5))
    }

    fn expect_change_within(
        &mut self,
        reference: &Image,
        region: Region,
        threshold: f64,
        stage: &str,
        timeout: Duration,
    ) -> Result<Image, Failure> {
        self.expect_pixels(reference, region, stage, |diff| diff >= threshold, timeout)
            .map_err(|miss| {
                miss.into_failure(|diff| format!("No visible change ({region:?} differed by {diff:.5}): {stage}"))
            })
    }

    /// Wait until `region` matches `reference` within `tolerance`.
    fn expect_same(
        &mut self,
        reference: &Image,
        region: Region,
        tolerance: f64,
        stage: &str,
    ) -> Result<Image, Failure> {
        self.expect_pixels(
            reference,
            region,
            stage,
            |diff| diff <= tolerance,
            Duration::from_secs(5),
        )
        .map_err(|miss| miss.into_failure(|diff| format!("Visible state differs ({region:?} by {diff:.5}): {stage}")))
    }

    fn expect_pixels(
        &mut self,
        reference: &Image,
        region: Region,
        stage: &str,
        accept: impl Fn(f64) -> bool,
        timeout: Duration,
    ) -> Result<Image, Miss> {
        let deadline = Instant::now() + timeout;
        loop {
            // A capture that fails is the desktop's failure, not a pixel mismatch.
            let image = match self.capture() {
                Ok(image) => image,
                Err(failure) => {
                    self.record(
                        stage,
                        serde_json::json!({"region": format!("{region:?}"), "capture_failure": failure.detail,
                            "class": failure.class.name()}),
                    );
                    return Err(Miss::Capture(failure));
                }
            };
            let rect = self.rect(region, &image);
            let last = image.diff_fraction(reference, rect);
            if accept(last) || Instant::now() >= deadline {
                let passed = accept(last);
                let _ = self.shot(stage);
                self.record(
                    stage,
                    serde_json::json!({"region": format!("{region:?}"), "rect": [rect.0, rect.1, rect.2, rect.3],
                        "diff_fraction": last, "accepted": passed}),
                );
                return if passed { Ok(image) } else { Err(Miss::Pixels(last)) };
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// The active tab shows (or no longer shows) its modified marker, compared
    /// with a capture of the same tab while clean.
    fn expect_dirty(&mut self, clean: &Image, dirty: bool, stage: &str) -> Result<(), Failure> {
        if dirty {
            self.expect_change(clean, Region::Tabs, 0.0002, stage).map(|_| ())
        } else {
            self.expect_same(clean, Region::Tabs, 0.0001, stage).map(|_| ())
        }
    }

    /// The tab strip of an editor showing a clean, loaded document named as
    /// `document`: a one-line file of that name, opened in an editor of its own,
    /// which loads before its first settled capture. A paged document's tab
    /// reads "name (loading)" until it has loaded, so matching this capture
    /// is the positive sign that it has.
    fn loaded_tab(&mut self, document: &Path) -> Result<Image, Failure> {
        let name = document
            .file_name()
            .ok_or_else(|| Failure::harness("a loaded tab needs a file name"))?;
        let file = self.scratch.join("loaded-tab").join(name);
        self.write(
            &file,
            b"loaded tab reference
",
        )?;
        self.launch(Launch::files(&[file.as_path()]))?;
        let image = self.settle(Duration::from_secs(5))?;
        let _ = self.shot(&format!("loaded tab of {}", name.to_string_lossy()));
        self.exit()?;
        Ok(image)
    }

    /// Wait until the owned editor's tab strip shows the document loaded:
    /// it matches `loaded`, the strip of a loaded document of the same name
    /// (`loaded_tab`). Until then the tab reads "name (loading)" and the
    /// document drops typed input.
    fn wait_loaded(&mut self, loaded: &Image, timeout: Duration) -> Result<Image, Failure> {
        let started = Instant::now();
        let result = self.expect_pixels(loaded, Region::Tabs, "document loaded", |diff| diff <= 0.0005, timeout);
        self.record(
            "document load wait",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis(), "tab_matched_loaded": result.is_ok()}),
        );
        result.map_err(|miss| {
            miss.into_failure(|diff| {
                format!(
                    "The document did not finish loading within {} s: its tab still differs from a loaded tab of                      the same name ({diff:.5})",
                    timeout.as_secs()
                )
            })
        })
    }

    /// Wait until two consecutive captures agree (progress, scrolling and
    /// incremental search have settled).
    fn settle(&mut self, timeout: Duration) -> Result<Image, Failure> {
        let deadline = Instant::now() + timeout;
        let mut previous = self.capture()?;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let current = self.capture()?;
            let rect = (0, 0, current.width, current.height);
            if current.diff_fraction(&previous, rect) < 0.0002 || Instant::now() >= deadline {
                return Ok(current);
            }
            previous = current;
        }
    }

    // ---------------------------------------------------------------- files ---

    fn expect_file(&mut self, path: &Path, expected: &[u8], stage: &str) -> Result<(), Failure> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let actual = std::fs::read(path).ok();
            if actual.as_deref() == Some(expected) || Instant::now() >= deadline {
                let matched = actual.as_deref() == Some(expected);
                let digest = actual.as_ref().map(|bytes| format!("{:x}", Sha256::digest(bytes)));
                self.record(
                    stage,
                    serde_json::json!({
                        "path": path,
                        "bytes": actual.as_ref().map(Vec::len),
                        "sha256": digest,
                        "expected_bytes": expected.len(),
                        "expected_sha256": format!("{:x}", Sha256::digest(expected)),
                        "matched": matched,
                        "actual_prefix": actual.as_ref().map(|bytes| String::from_utf8_lossy(&bytes[..bytes.len().min(160)]).into_owned()),
                    }),
                );
                return if matched {
                    Ok(())
                } else {
                    Err(Failure::product(format!("Exact saved bytes mismatch: {stage}")))
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    // -------------------------------------------------------------- services ---

    /// Classify a missing service: `service_not_wired` while the seam holds
    /// its stand-in, otherwise a product failure.
    fn needs(&self, service: Service, detail: String) -> Failure {
        match steps::seam_stand_in(&self.env.root, service) {
            Some(true) => Failure::not_wired(
                service,
                format!("{detail} (the shell seam for {} is still the stand-in)", service.name()),
            ),
            _ => Failure::product(detail),
        }
    }

    /// Open a native file or folder chooser with `opener` and choose `target`
    /// in it. Only a window that appeared after the opener and belongs to the
    /// editor or to a file-chooser host (an XDG desktop portal backend, the
    /// macOS open and save panel service) is taken for the dialog, so no input
    /// ever reaches a window that was already on the desktop or one of another
    /// application. Without a chooser the step is classified: the dialog
    /// service is not wired, or this session cannot host dialogs at all.
    fn choose_in_dialog(&mut self, opener: Opener<'_>, target: &Path) -> StepResult {
        let label = opener.label();
        let editor = self.pid()?;
        if let Some(answered) = self.desktop.answer_next_chooser(target)? {
            return self.choose_through_portal(opener, target, answered);
        }
        let before = self.desktop.all_windows()?;
        self.open_chooser(opener)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut ignored: Vec<Window> = Vec::new();
        let dialog = loop {
            let (dialog, foreign) = pick_dialog(&before, self.desktop.all_windows()?, |window| {
                dialog_owner(window, editor)
            });
            for window in foreign {
                if !ignored.iter().any(|known| known.id == window.id) {
                    ignored.push(window);
                }
            }
            if dialog.is_some() || Instant::now() >= deadline {
                break dialog;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if !ignored.is_empty() {
            self.record(
                "foreign windows ignored",
                serde_json::json!({"opener": label, "windows": ignored.iter().map(Window::json).collect::<Vec<_>>()}),
            );
        }
        let Some(dialog) = dialog else {
            let _ = self.shot(&format!("{label} no dialog"));
            let host = self.desktop.dialog_host();
            self.record(
                "file dialog missing",
                serde_json::json!({"opener": label, "dialog_host": format!("{host:?}")}),
            );
            let detail = format!("{label}: no file dialog appeared within 8 s");
            return Err(match (steps::seam_stand_in(&self.env.root, Service::Dialogs), host) {
                (Some(true), _) => Stop::Fail(self.needs(Service::Dialogs, detail)),
                (_, Err(why)) => Stop::Skip(format!(
                    "{label} needs a native file dialog and this session cannot show one: {why}"
                )),
                _ => Stop::Fail(Failure::product(format!("Native file dialog did not appear: {detail}"))),
            });
        };
        // A portal file chooser (GTK and KDE alike) takes a typed location.
        self.record(
            "file dialog",
            serde_json::json!({"opener": label, "window": dialog.json(), "target": target}),
        );
        self.desktop.focus(&dialog)?;
        for chord in ["Primary+L", "Primary+A"] {
            self.desktop.key(&dialog, chord)?;
        }
        self.desktop.text(&dialog, &target.to_string_lossy())?;
        self.desktop.key(&dialog, "Return")?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.desktop.all_windows()?.iter().any(|window| window.id == dialog.id) {
            if Instant::now() >= deadline {
                return Err(Stop::Fail(Failure::timeout(
                    "File dialog did not dismiss after one submission",
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let window = self.window()?;
        self.desktop.focus(&window)?;
        Ok(())
    }

    fn open_chooser(&mut self, opener: Opener<'_>) -> Result<(), Failure> {
        match opener {
            Opener::Command(title) => self.command(title),
            Opener::Key { chord, .. } => self.key(chord),
        }
    }

    /// The desktop's portal stand-in answers the chooser with `target` (staged
    /// before the opener ran): the step requires the editor to ask the portal
    /// for exactly one chooser, and lists the chooser's own UI as substituted.
    fn choose_through_portal(&mut self, opener: Opener<'_>, target: &Path, answered: usize) -> StepResult {
        let label = opener.label();
        self.open_chooser(opener)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let request = loop {
            let requests = self.desktop.chooser_requests();
            if requests.len() > answered || Instant::now() >= deadline {
                break requests.into_iter().nth(answered);
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let Some(request) = request else {
            let _ = self.shot(&format!("{label} no dialog"));
            self.record(
                "file dialog missing",
                serde_json::json!({"opener": label, "dialog_host": format!("{:?}", self.desktop.dialog_host())}),
            );
            return Err(Stop::Fail(self.needs(
                Service::Dialogs,
                format!("{label}: the editor asked the session's portal for no file chooser within 8 s"),
            )));
        };
        self.record(
            "file dialog",
            serde_json::json!({"opener": label, "portal_request": request, "target": target}),
        );
        self.substituted(
            &format!("{label} chooser window"),
            "the harness's portal stand-in on a private session bus answered the editor's FileChooser request with              the target; no chooser window was driven",
        );
        if request["response"] != 0 {
            return Err(Stop::Fail(Failure::harness(format!(
                "{label}: the portal stand-in had no staged answer for the request"
            ))));
        }
        // The editor retires its "Waiting for the file dialog" modal once the
        // answer arrives; the step's own oracle checks what it did with it.
        std::thread::sleep(Duration::from_millis(500));
        let window = self.window()?;
        self.desktop.focus(&window)?;
        Ok(())
    }

    /// The lines of the owned editor's standard error that start with `prefix`
    /// (the shell's `event=` diagnostics, such as `event=prompt_shown`).
    fn stderr_events(&mut self, prefix: &str) -> Result<Vec<String>, Failure> {
        let path = self.editor()?.stderr.clone();
        Ok(std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .map(str::to_owned)
            .collect())
    }

    // -------------------------------------------------------------- process ---

    /// Private memory of the editor in KiB (Linux RssAnon, macOS resident set).
    fn private_kib(&mut self) -> Result<u64, Failure> {
        let pid = self.pid()?;
        private_kib(pid).ok_or_else(|| Failure::harness("process memory unavailable"))
    }

    fn profile_files(&self, roots: &[PathBuf]) -> Vec<String> {
        let mut found = Vec::new();
        for root in roots {
            walk_files(root, root, &mut found, 4096);
        }
        found.sort();
        found
    }
}

/// A stand-in reference for steps that never ran their capture.
/// Whether a capture shows more than one colour (a window that has presented
/// its content, not a still black or blank surface).
fn painted(image: &Image) -> bool {
    let (pixels, _) = image.pixels.as_chunks::<4>();
    let mut colours = pixels.iter().map(|pixel| &pixel[..3]);
    colours
        .next()
        .is_some_and(|first| colours.any(|colour| colour != first))
}

fn blank() -> Image {
    Image {
        width: 0,
        height: 0,
        pixels: Vec::new(),
    }
}

/// The first-frame event the editor prints on its standard output.
fn first_frame_event(stdout: &Path) -> Option<serde_json::Value> {
    std::fs::read_to_string(stdout).ok().and_then(|text| {
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| value["event"] == "first_frame")
    })
}

/// What opens a native file chooser.
#[derive(Clone, Copy)]
enum Opener<'a> {
    /// A command run by its exact title through the command palette.
    Command(&'a str),
    /// A key chord sent to the editor; `label` names it in the evidence.
    Key { chord: &'a str, label: &'a str },
}

impl<'a> Opener<'a> {
    fn label(self) -> &'a str {
        match self {
            Opener::Command(title) => title,
            Opener::Key { label, .. } => label,
        }
    }
}

/// Split the windows `now` on the desktop that were not in `before` into the
/// first one `owner` accepts (the dialog) and the rest, which belong to other
/// applications and must never receive input.
fn pick_dialog(before: &[Window], now: Vec<Window>, owner: impl Fn(&Window) -> bool) -> (Option<Window>, Vec<Window>) {
    let (owned, foreign): (Vec<Window>, Vec<Window>) = now
        .into_iter()
        .filter(|window| !before.iter().any(|known| known.id == window.id))
        .partition(owner);
    (owned.into_iter().next(), foreign)
}

/// Whether `window` can be a chooser opened for the editor process `editor`:
/// one of the editor's own windows (GTK and AppKit panels run in process), or
/// one of a process that hosts choosers for other applications.
fn dialog_owner(window: &Window, editor: u32) -> bool {
    window
        .pid
        .is_some_and(|pid| pid == editor || process_name(pid).is_some_and(|name| is_chooser_host(&name)))
}

/// `ps -o comm=` of a process: Linux prints the name truncated to 15 bytes,
/// macOS the executable's path.
fn process_name(pid: u32) -> Option<String> {
    let output = ps(pid, "comm=")?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The processes that show file choosers for other applications: the XDG
/// desktop portal backends (xdg-desktop-portal-gtk, -gnome, -kde, ...) and
/// the macOS open and save panel service.
fn is_chooser_host(command: &str) -> bool {
    let name = command.rsplit('/').next().unwrap_or(command);
    name.starts_with("xdg-desktop-por") || name.contains("openAndSavePanelService")
}

/// The desktop session bus for the editor: the harness moves XDG_RUNTIME_DIR
/// into the scratch home, so name the real bus explicitly when it is only
/// found there.
fn session_bus() -> Option<std::ffi::OsString> {
    if let Some(address) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
        return Some(address);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let bus = Path::new(&runtime).join("bus");
    bus.exists().then(|| format!("unix:path={}", bus.display()).into())
}

fn private_kib(pid: u32) -> Option<u64> {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status
            .lines()
            .find_map(|line| line.strip_prefix("RssAnon:"))
            .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
    } else {
        let output = ps(pid, "rss=")?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
}

fn parent_pid(pid: u32) -> Option<u32> {
    let output = ps(pid, "ppid=")?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// One `ps -o <field> -p <pid>` with a deadline, so a wedged `ps` cannot hold
/// a journey until the job's timeout.
fn ps(pid: u32, field: &str) -> Option<Output> {
    output_within(
        Command::new("ps").args(["-o", field, "-p", &pid.to_string()]),
        "ps",
        Duration::from_secs(10),
    )
    .ok()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn walk_files(root: &Path, directory: &Path, found: &mut Vec<String>, limit: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= limit {
            return;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk_files(root, &path, found, limit);
        } else {
            found.push(path.strip_prefix(root).unwrap_or(&path).display().to_string());
        }
    }
}

/// `(relative path, sha256, bytes)` of every file below `root`, refusing links.
fn inventory(root: &Path) -> Result<Vec<(String, String, u64)>, Failure> {
    let mut files = Vec::new();
    walk_files(root, root, &mut files, 4096);
    files.sort();
    files
        .into_iter()
        .map(|relative| {
            let path = root.join(&relative);
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|error| Failure::harness(format!("{relative}: {error}")))?;
            if metadata.file_type().is_symlink() {
                return Err(Failure::harness(format!("Portable inventory path refused: {relative}")));
            }
            let bytes = std::fs::read(&path).map_err(|error| Failure::harness(format!("{relative}: {error}")))?;
            Ok((relative, format!("{:x}", Sha256::digest(&bytes)), metadata.len()))
        })
        .collect()
}

/// Count pixels near each probe colour (±12 per channel) inside `rect`.
fn colour_counts(image: &Image, rect: (i32, i32, i32, i32), probes: &[(String, u32)]) -> Vec<(String, u64)> {
    let near = |(r, g, b): (u8, u8, u8), rgb: u32| {
        [(r, 16), (g, 8), (b, 0)]
            .iter()
            .all(|(value, shift)| (i32::from(*value) - ((rgb >> shift) & 255) as i32).abs() <= 12)
    };
    probes
        .iter()
        .map(|(kind, rgb)| {
            let mut count = 0u64;
            for y in rect.1.max(0)..rect.3.min(image.height) {
                for x in rect.0.max(0)..rect.2.min(image.width) {
                    if near(image.rgb(x, y), *rgb) {
                        count += 1;
                    }
                }
            }
            (kind.clone(), count)
        })
        .collect()
}

// ---------------------------------------------------------------- the smoke --

fn journey_smoke(env: &Env) -> Result<(), String> {
    let (home, data_root) = env.scratch("smoke")?;
    let layout = Layout::new(&home);
    layout.create().map_err(|failure| failure.detail)?;
    // As on Windows: a marked diagnostic root keeps the smoke out of any profile.
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
    for (name, value) in layout.environment() {
        command.env(name, value);
    }
    let capture = crate::capture::run(&mut command, Duration::from_secs(30));
    layout.remove_runtime();
    let capture = capture.map_err(|e| e.to_string())?;
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

// --------------------------------------------------------------- procedures --

const SESSION_ROUTE: &str = "a launch that keeps the session runs without --new-instance and --no-extensions,      which make the window independent of the profile's session; the attempt's private profile and runtime folder      leave no other window to join";
const OPEN_ROUTE: &str = "Open needs the dialogs service; the file was given on the editor's command line";

fn plain_text(run: &mut Run) -> Result<(), Failure> {
    let first = "A\u{1F389}e\u{301}\u{6587}";
    let second = "second line";
    // An existing empty file has no line ending to keep; the editor gives it
    // its default (LF), not files.default_eol, which applies to new documents.
    let expected = format!("{first}\n{second}");
    let edited = format!("{expected}!");
    run.fixture = serde_json::json!({"new_file_eol": "lf", "new_file_encoding": "utf-8",
        "route": "empty scratch file on the command line"});
    let saved = run.scratch.join("plain-unicode.txt");
    run.write(&saved, b"")?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "lf")?;
    let mut clean = None;
    run.step(
        "s1",
        "An empty scratch document opened from the command line accepted astral, combining and CJK Unicode plus Enter; the body changed and the tab shows its modified marker.",
        |run| {
            run.substituted(
                "fresh Untitled document",
                "Save As needs the dialogs service; an empty scratch file was opened from the command line instead",
            );
            run.launch(Launch::files(&[saved.as_path()]))?;
            let reference = run.shot("plain clean")?;
            run.text(first)?;
            run.key("Return")?;
            run.text(second)?;
            run.expect_change(&reference, Region::Body, 0.0005, "Unicode and line break entered")?;
            run.expect_dirty(&reference, true, "new document modified")?;
            run.unobservable("exact document text and modified tab name");
            clean = Some(reference);
            Ok(())
        },
    );
    let clean = clean.unwrap_or_else(blank);

    run.step(
        "s2",
        "Save wrote exact UTF-8 LF bytes and cleared the modified marker; Close removed the document; a relaunch with the file restored it clean without rewriting its bytes.",
        |run| {
            run.key("Primary+S")?;
            run.expect_file(&saved, expected.as_bytes(), "first save exact bytes")?;
            run.expect_dirty(&clean, false, "saved tab clean")?;
            let before = run.shot("before close")?;
            run.key("Primary+W")?;
            run.expect_change(&before, Region::Tabs, 0.004, "document closed")?;
            run.unobservable("closed document tab absent and empty editor text");
            run.exit()?;
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            run.expect_dirty(&clean, false, "reopened tab clean")?;
            run.expect_file(&saved, expected.as_bytes(), "reopen did not rewrite bytes")?;
            run.unobservable("reopened exact text");
            Ok(())
        },
    );
    run.step(
        "s3",
        "One edit, Undo and Redo toggled the modified marker; unsaved edits did not alter disk bytes; saving after Redo wrote the edit and a final Undo restored the original bytes.",
        |run| {
            run.keys(&["Primary+End"])?;
            run.text("!")?;
            run.expect_dirty(&clean, true, "edit dirty")?;
            run.key("Primary+Z")?;
            run.expect_dirty(&clean, false, "Undo clean")?;
            run.key("Primary+Y")?;
            run.expect_dirty(&clean, true, "Redo dirty")?;
            run.expect_file(&saved, expected.as_bytes(), "unsaved edit never changes disk")?;
            // The saved bytes stand in for the Windows driver's text reads.
            run.key("Primary+S")?;
            run.expect_file(&saved, edited.as_bytes(), "redone edit saved")?;
            run.key("Primary+Z")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, expected.as_bytes(), "cleanup Undo saved")?;
            run.expect_dirty(&clean, false, "cleanup clean")?;
            run.unobservable("exact text after each Undo and Redo");
            Ok(())
        },
    );
    Ok(())
}

/// Pixel counts of each token colour in the editor body; at least three
/// pixels of every probe colour must be painted.
fn observe_highlighting(run: &mut Run, fixture: &serde_json::Value, stage: &str) -> Result<(), Failure> {
    let probes: Vec<(String, u32)> = fixture["probes"]
        .as_array()
        .ok_or_else(|| Failure::harness("fixture probes missing"))?
        .iter()
        .filter_map(|probe| Some((probe["kind"].as_str()?.to_owned(), probe["rgb"].as_u64()? as u32)))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let image = run.capture()?;
        let rect = run.rect(Region::Body, &image);
        let counts = colour_counts(&image, rect, &probes);
        let painted = counts.iter().all(|(_, count)| *count >= 3);
        if painted || Instant::now() >= deadline {
            let _ = run.shot(stage);
            run.record(
                stage,
                serde_json::json!({"probes": counts.iter().map(|(kind, count)| serde_json::json!({"kind": kind, "matched_pixels": count})).collect::<Vec<_>>(),
                    "rect": [rect.0, rect.1, rect.2, rect.3], "max_saturation": image.max_saturation(rect)}),
            );
            return if painted {
                Ok(())
            } else {
                Err(Failure::product(
                    "Rendered token colors did not match the selected language/theme",
                ))
            };
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn code_config(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('code_config_fixture').fixture('dark')")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let last = Run::fixture_text(&fixture, "final")?;
    let saved = run.scratch.join("code-config.rs");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let mut clean = None;
    run.step(
        "s1",
        "Generated UTF-8 Rust opened unchanged; keyword, string, number and comment pixels carry the selected theme's token colours.",
        |run| {
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            clean = Some(run.shot("code opened")?);
            run.unobservable("exact text and 'Language: Rust' status");
            run.command("Unfold All")?;
            observe_highlighting(run, &fixture, "code highlighting")?;
            run.gap(
                "token rectangles",
                GapKind::Unobservable(Service::Accessibility),
                "UI Automation bounds locate each token on Windows; here every probe colour is counted over the editor body",
            );
            run.expect_file(&saved, initial.as_bytes(), "code source initially unchanged")?;
            Ok(())
        },
    );
    let clean = clean.unwrap_or_else(blank);

    run.step(
        "s2",
        "Enter kept the indentation, Ctrl+Space offered completions, Down and Enter accepted one, Undo and Redo ran once each and ';' was typed; unsaved disk bytes stayed unchanged.",
        |run| {
            run.keys(&["Primary+End", "Up", "End", "Return"])?;
            run.text("ans")?;
            let before = run.shot("code completion prefix")?;
            run.key("Primary+Space")?;
            run.expect_change(&before, Region::Body, 0.002, "code completion offered")?;
            run.key("Down")?;
            run.key("Return")?;
            run.key("Primary+Z")?;
            run.key("Primary+Y")?;
            run.text(";")?;
            run.expect_dirty(&clean, true, "code edited dirty")?;
            run.expect_file(&saved, initial.as_bytes(), "code unsaved edits preserve disk")?;
            run.unobservable("indented, prefix, completed, undone and redone text; the selected completion item");
            Ok(())
        },
    );
    run.step(
        "s3",
        "Save wrote the independently expected source (proving the indentation, completion, Undo and Redo), cleared the marker; Close removed the tab and a relaunch reopened it without rewriting bytes.",
        |run| {
            run.key("Primary+S")?;
            run.expect_file(&saved, last.as_bytes(), "code saved exact bytes")?;
            run.expect_dirty(&clean, false, "code saved clean")?;
            let before = run.shot("code before close")?;
            run.key("Primary+W")?;
            run.expect_change(&before, Region::Tabs, 0.004, "code document closed")?;
            run.exit()?;
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            run.expect_dirty(&clean, false, "code reopened clean")?;
            run.expect_file(&saved, last.as_bytes(), "code reopened bytes unchanged")?;
            Ok(())
        },
    );
    Ok(())
}

fn regex_transform(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('regex_transform_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let replaced = Run::fixture_text(&fixture, "replaced")?;
    let pattern = Run::fixture_text(&fixture, "pattern")?;
    let replacement = Run::fixture_text(&fixture, "replacement")?;
    let saved = run.scratch.join("fixture/input.txt");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let mut clean = None;
    run.step(
        "s1",
        "The multiline Unicode source opened unchanged; regular-expression mode and the Replace panel took the exact PCRE2 query without touching the document.",
        |run| {
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            let reference = run.shot("regex opened")?;
            run.command("Regular Expression Search Mode")?;
            run.gap("regex mode menu item checked", GapKind::WindowsOnly, steps::NATIVE_MENU_STATE);
            run.command("Replace\u{2026}")?;
            run.text(&pattern)?;
            run.expect_change(&reference, Region::Body, 0.002, "regex replace panel with query")?;
            run.unobservable("Find field value and 'Find results: 2 matches'");
            run.expect_file(&saved, initial.as_bytes(), "regex setup preserved bytes")?;
            clean = Some(reference);
            Ok(())
        },
    );
    let clean = clean.unwrap_or_else(blank);

    run.step(
        "s2",
        "Named and numbered captures replaced exactly two multiline matches through Replace All in Current Document (the Replace in Files preview was not exercised); Save wrote the exact expected UTF-8/LF bytes.",
        |run| {
            run.key("Tab")?;
            run.text(&replacement)?;
            // A different feature, not a different route: reported as reduced coverage.
            run.gap(
                "Replace in Files preview and Apply",
                GapKind::NotCovered(Service::Dialogs),
                "the preview needs the folder chooser (dialogs service); Replace All in Current Document applied the same pattern and replacement to the open document instead",
            );
            run.command("Replace All in Current Document")?;
            run.key("Escape")?;
            run.expect_dirty(&clean, true, "regex replaced dirty")?;
            run.expect_file(&saved, initial.as_bytes(), "regex unsaved replacement preserved disk")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, replaced.as_bytes(), "regex saved replacement bytes")?;
            run.expect_dirty(&clean, false, "regex replaced saved clean")?;
            run.retain(&saved, "regex-replaced.bin");
            run.unobservable("preview rows and replacement count status");
            Ok(())
        },
    );
    run.step(
        "s3",
        "One Undo restored the whole original document; disk kept the replacement until Save, which restored the original Unicode and LF bytes.",
        |run| {
            run.key("Primary+Z")?;
            run.expect_dirty(&clean, true, "regex Undo dirty")?;
            run.expect_file(&saved, replaced.as_bytes(), "regex Undo before save preserved disk")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, initial.as_bytes(), "regex restored original bytes")?;
            run.expect_dirty(&clean, false, "regex restored clean")?;
            Ok(())
        },
    );
    Ok(())
}

fn column_multi_cursor(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('column_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let inserted = Run::fixture_text(&fixture, "inserted")?;
    let text = Run::fixture_text(&fixture, "text")?;
    let saved = run.scratch.join("column.txt");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let mut clean = None;
    run.step(
        "s1",
        "Two Alt+Shift+Down presses from display column four added carets across a tab, a wide glyph and a short line; source bytes remain exact.",
        |run| {
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            clean = Some(run.shot("column opened")?);
            run.keys(&["Primary+Home", "Right", "Right", "Alt+Shift+Down", "Alt+Shift+Down"])?;
            let _ = run.shot("column carets")?;
            run.unobservable("three empty selections at UTF-16 offsets 2, 7 and 10");
            run.expect_file(&saved, initial.as_bytes(), "column original disk")?;
            Ok(())
        },
    );
    let clean = clean.unwrap_or_else(blank);

    run.step(
        "s2",
        "One Column Editor apply inserted at every display column and padded the short line; Save wrote the exact bytes.",
        |run| {
            run.command("Column Editor\u{2026}")?;
            // The Repeated text field is the first field after the mode buttons.
            run.key("Tab")?;
            run.text(&text)?;
            let _ = run.shot("column insertion input")?;
            run.key("Return")?;
            run.expect_dirty(&clean, true, "column inserted dirty")?;
            run.expect_file(&saved, initial.as_bytes(), "column unsaved disk")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, inserted.as_bytes(), "column inserted disk")?;
            run.retain(&saved, "column-inserted.bin");
            Ok(())
        },
    );
    run.step(
        "s3",
        "One Undo restored every original row in one transaction; Save restored the original UTF-8/LF bytes.",
        |run| {
            run.key("Primary+Z")?;
            run.expect_file(&saved, inserted.as_bytes(), "column Undo disk unchanged")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, initial.as_bytes(), "column restored disk")?;
            run.expect_dirty(&clean, false, "column restored clean")?;
            Ok(())
        },
    );
    Ok(())
}

fn huge_log_tail(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('huge_log_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let saved = run.scratch.join("huge-log.txt");
    let generated = run.python(&format!(
        "__import__('huge_log_fixture').generate(__import__('pathlib').Path({:?}))",
        saved.to_string_lossy()
    ))?;
    run.record("log generated", generated);
    let limit = fixture["private_growth_limit"].as_u64().unwrap_or(192 * 1024 * 1024) / 1024;
    let needle = Run::fixture_text(&fixture, "needle")?;
    let append = Run::fixture_text(&fixture, "append")?;
    let rotated = Run::fixture_text(&fixture, "rotated")?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    run.step(
        "s1",
        "The generated 512 MiB log opened as an editable viewport; one edit changed it, one Undo restored the same pixels and a clean tab, and private-memory growth stayed below 192 MiB.",
        |run| {
            run.substituted(
                "native Open and its memory baseline",
                "the log was opened from the command line; the baseline is a fresh editor without documents",
            );
            run.launch(Launch::files(&[]))?;
            let baseline = run.private_kib()?;
            run.exit()?;
            let loaded = run.loaded_tab(&saved)?;
            run.launch(Launch::files(&[saved.as_path()]))?;
            run.wait_loaded(&loaded, Duration::from_secs(60))?;
            run.key("Primary+Home")?;
            let reference = run.settle(Duration::from_secs(5))?;
            run.text("X")?;
            // One inserted character, not a reloading viewport; as on Windows
            // (Log-View), each state may take up to 20 s to show.
            run.expect_pixels(
                &reference,
                Region::Body,
                "log edited viewport",
                |diff| (0.0003..=0.03).contains(&diff),
                Duration::from_secs(20),
            )
            .map_err(|miss| {
                miss.into_failure(|diff| {
                    format!("The edited viewport did not show one inserted character within 20 s (Body differed by {diff:.5})")
                })
            })?;
            run.key("Primary+Z")?;
            run.expect_pixels(&reference, Region::Body, "log Undo viewport", |diff| diff <= 0.0003, Duration::from_secs(20))
                .map_err(|miss| {
                    miss.into_failure(|diff| {
                        format!("Undo did not restore the viewport within 20 s (Body differed by {diff:.5})")
                    })
                })?;
            run.expect_dirty(&reference, false, "log Undo clean")?;
            let after = run.private_kib()?;
            run.record(
                "log memory bound",
                serde_json::json!({"baseline_kib": baseline, "after_kib": after, "limit_kib": limit}),
            );
            run.unobservable("first viewport text '0123456789abcdef'");
            if after.saturating_sub(baseline) > limit {
                return Err(Failure::product("Log private-memory growth exceeded bound").into());
            }
            Ok(())
        },
    );
    run.step(
        "s2",
        "Literal search for the Unicode marker that crosses the generated boundary completed and Enter moved the viewport to it.",
        |run| {
            run.command("Literal Search Mode")?;
            run.key("Primary+F")?;
            run.key("Primary+A")?;
            run.text(&needle)?;
            // The complete search reports its count in the find bar; Windows
            // waits for 'Find results: 1 matches' before Enter.
            std::thread::sleep(Duration::from_millis(500));
            let searching = run.capture()?;
            let _ = run.expect_change_within(
                &searching,
                Region::FindStatus,
                0.01,
                "log search status changed",
                Duration::from_secs(40),
            );
            let searched = run.settle(Duration::from_secs(10))?;
            run.unobservable("'Find results: 1 matches' and the selected marker text");
            run.key("Return")?;
            // Enter while the complete search still runs selects the match
            // once it is found ("Preparing selected text…").
            run.expect_change_within(&searched, Region::Body, 0.01, "log boundary selection", Duration::from_secs(45))?;
            run.key("Escape")?;
            Ok(())
        },
    );
    run.step(
        "s3",
        "Monitoring showed a flushed append; rotating the owned fixture and reopening followed the replacement without stale content.",
        |run| {
            run.command("Follow New Content")?;
            let before = run.settle(Duration::from_secs(10))?;
            {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&saved)
                    .map_err(|error| Failure::harness(error.to_string()))?;
                file.write_all(append.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|error| Failure::harness(error.to_string()))?;
            }
            // Only a pixel miss points at the watcher; a failed capture or a
            // harness fault keeps its own class.
            if let Err(failure) = run.expect_change(&before, Region::Body, 0.002, "log appended viewport") {
                if failure.class != Class::Product {
                    return Err(failure.into());
                }
                return Err(run
                    .needs(
                        Service::FileWatching,
                        format!("Follow New Content did not show the flushed append ({})", failure.detail),
                    )
                    .into());
            }
            let old = run.scratch.join("log-rotated.txt");
            std::fs::rename(&saved, old).map_err(|error| Failure::harness(error.to_string()))?;
            run.write(&saved, rotated.as_bytes())?;
            std::thread::sleep(Duration::from_secs(2));
            let before = run.capture()?;
            run.command("Reopen and Follow")?;
            run.expect_change(&before, Region::Body, 0.002, "log rotated viewport")?;
            run.expect_file(&saved, rotated.as_bytes(), "log rotated bytes")?;
            run.unobservable("appended and rotated viewport text");
            Ok(())
        },
    );
    Ok(())
}

const ACCESSIBILITY_ROWS: &str = "focuses tree, outline or output rows through the accessibility tree (UI Automation \
     SetFocus on Windows); this platform publishes no accessibility tree yet";

fn workspace(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('workspace_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let root = run.scratch.join("workspace-fixture");
    let main = root.join("main.rs");
    run.write(&main, Run::fixture_text(&fixture, "initial")?.as_bytes())?;
    run.write(
        &root.join("notes.txt"),
        Run::fixture_text(&fixture, "notes")?.as_bytes(),
    )?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    run.step(
        "s1",
        "An explicitly chosen scratch workspace opened in the workspace panel.",
        |run| {
            run.launch(Launch::files(&[]))?;
            let before = run.shot("workspace empty")?;
            run.choose_in_dialog(Opener::Command("Open Workspace Folder\u{2026}"), &root)?;
            run.expect_change(&before, Region::Body, 0.002, "workspace tree shown")?;
            run.unobservable("exactly the two generated rows");
            Ok(())
        },
    );
    run.step("s2", "", |_| {
        Err(Stop::Skip(format!("Tree activation {ACCESSIBILITY_ROWS}")))
    });
    run.step("s3", "", |_| {
        Err(Stop::Skip(format!("Rename and re-navigation {ACCESSIBILITY_ROWS}")))
    });
    Ok(())
}

fn udl(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('udl_fixture').fixture('dark')")?;
    run.fixture = fixture["identity"].clone();
    let sample = run.scratch.join(Run::fixture_text(&fixture, "file_name")?);
    let initial = Run::fixture_text(&fixture, "initial")?;
    run.write(&sample, initial.as_bytes())?;
    let xml = run.scratch.join("fixture-udl.xml");
    run.write(&xml, Run::fixture_text(&fixture, "xml")?.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let definition = run.layout.profile.join("languages/qa-fixture.json");
    run.step(
        "s1",
        "The Notepad++ UDL import installed the validated definition durably under the isolated profile.",
        |run| {
            run.launch(Launch::files(&[]))?;
            run.choose_in_dialog(Opener::Command("Import User-defined Language\u{2026}"), &xml)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !definition.is_file() {
                if Instant::now() >= deadline {
                    return Err(Failure::product("Durable UDL definition missing or excessive").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            run.retain(&definition, "udl-definition.json");
            run.unobservable("import report status");
            run.key("Escape")?;
            Ok(())
        },
    );
    run.step(
        "s2",
        "Opening the matching extension painted the installed keyword, string and number styles.",
        |run| {
            run.exit()?;
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[sample.as_path()]))?;
            observe_highlighting(run, &fixture, "udl highlighting")?;
            Ok(())
        },
    );
    run.step(
        "s3",
        "A fresh editor loaded the installed catalog and reapplied the imported language to the matching extension.",
        |run| {
            run.exit()?;
            run.launch(Launch::files(&[sample.as_path()]))?;
            observe_highlighting(run, &fixture, "udl restarted highlighting")?;
            run.expect_file(&sample, initial.as_bytes(), "udl source unchanged")?;
            Ok(())
        },
    );
    Ok(())
}

fn macro_external(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('macro_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let last = Run::fixture_text(&fixture, "final")?;
    let saved = run.scratch.join("macro-source.txt");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let macro_file = run.layout.profile.join("macros/macro-01.toml");
    run.step(
        "s1",
        "Recording captured a Unicode-safe edit and a literal search with explicit arguments; the saved macro and the exact transformed document were retained.",
        |run| {
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            run.key("Primary+Home")?;
            run.command("Start Macro Recording")?;
            run.text(&Run::fixture_text(&fixture, "prefix")?)?;
            run.command("Literal Search Mode")?;
            run.key("Primary+F")?;
            run.key("Primary+A")?;
            run.text(&Run::fixture_text(&fixture, "query")?)?;
            let _ = run.settle(Duration::from_secs(5))?;
            run.unobservable("'Find results: 1 matches'");
            run.keys(&["Return", "Escape"])?;
            run.text(&Run::fixture_text(&fixture, "replacement")?)?;
            run.command("Stop Macro Recording")?;
            run.command("Save Macros")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !std::fs::metadata(&macro_file).is_ok_and(|metadata| metadata.len() > 0) {
                if Instant::now() >= deadline {
                    return Err(Failure::product("Recorded macro was not saved").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            run.retain(&macro_file, "macro-01.toml");
            run.key("Primary+S")?;
            run.expect_file(&saved, last.as_bytes(), "macro recorded saved")?;
            Ok(())
        },
    );
    run.step(
        "s2",
        "A fresh editor reloaded the persisted macro and replayed it against the reset fixture with the exact expected result.",
        |run| {
            run.exit()?;
            run.write(&saved, initial.as_bytes())?;
            run.launch(Launch::files(&[saved.as_path()]))?;
            run.key("Primary+Home")?;
            run.command("Play Selected Macro")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, last.as_bytes(), "macro replay saved")?;
            Ok(())
        },
    );
    run.step(
        "s3",
        "A confirmed direct command preserved literal argv and its parent and descendant ended when the editor cancelled it.",
        |run| {
            let python = run.python("__import__('sys').executable")?;
            let definition = run.python(&format!(
                "__import__('macro_fixture').external_definition(__import__('pathlib').Path({:?}))",
                run.scratch.to_string_lossy()
            ))?;
            let definition = definition.as_str().unwrap_or_default().as_bytes();
            let toml = run.scratch.join("external-command.toml");
            run.write(&toml, definition)?;
            run.retain(&toml, "external-command.toml");
            run.record("external fixture", serde_json::json!({"python": python}));
            // The Windows route loads the definition through a native file
            // dialog. Without a wired chooser, or in a session that cannot host
            // one (no portal and no stand-in, BARELINE_QA_PORTAL=none), the definition
            // goes where the macro library loads it at startup and the owned
            // editor is relaunched on the same source file; the consent, argv,
            // process tree and cancellation checks below run unchanged.
            let host = run.desktop.dialog_host();
            let wired = steps::seam_stand_in(&run.env.root, Service::Dialogs) == Some(false);
            if wired && host.is_ok() {
                run.choose_in_dialog(Opener::Command("Load External Command Definition\u{2026}"), &toml)?;
            } else {
                let library = run.layout.profile.join(steps::EXTERNAL_DEFINITION);
                run.exit()?;
                run.write(&library, definition)?;
                run.launch(Launch::files(&[saved.as_path()]))?;
                run.substituted(
                    "Load External Command Definition (native dialog)",
                    "the definition was placed in the profile's macro library, which the editor loads at startup",
                );
                run.record(
                    "external definition route",
                    serde_json::json!({"library": library, "dialogs_wired": wired, "dialog_host": format!("{host:?}")}),
                );
            }
            let command = format!("Run {}", Run::fixture_text(&fixture, "command_name")?);
            let python = python.as_str().unwrap_or_default().to_owned();
            accept_external_consent(run, &command, &python)?;
            let receipt = run.scratch.join("external-receipt.json");
            let deadline = Instant::now() + Duration::from_secs(8);
            while !receipt.is_file() {
                if Instant::now() >= deadline {
                    return Err(run
                        .needs(
                            Service::ExternalProcesses,
                            "External command consent was accepted but the command never ran".into(),
                        )
                        .into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            run.retain(&receipt, "external-receipt.json");
            let value: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&receipt).map_err(|error| Failure::harness(error.to_string()))?,
            )
            .map_err(|error| Failure::harness(error.to_string()))?;
            let parent = value["pid"].as_u64().unwrap_or(0) as u32;
            let child = value["child_pid"].as_u64().unwrap_or(0) as u32;
            let editor = run.pid()?;
            run.gap("Win32 process identities", GapKind::WindowsOnly, steps::WIN32_PROCESS_TREE);
            run.record(
                "external literal argv",
                serde_json::json!({"receipt": value, "parent_ppid": parent_pid(parent), "child_ppid": parent_pid(child)}),
            );
            if parent_pid(parent) != Some(editor) || parent_pid(child) != Some(parent) {
                return Err(Failure::product("External process identities are outside the owned editor tree").into());
            }
            if value["argv"] != fixture["argv"] {
                return Err(Failure::product("External command argv was not preserved literally").into());
            }
            run.gap("output link navigation", GapKind::Unobservable(Service::Accessibility), ACCESSIBILITY_ROWS);
            run.command("Cancel External Command")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while alive(parent) || alive(child) {
                if Instant::now() >= deadline {
                    return Err(Failure::product("External cancellation left an owned process alive").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        },
    );
    Ok(())
}

/// How the external-command consent appeared.
enum Consent {
    /// The shell's in-app prompt, with its `event=prompt_shown` line.
    InApp(String),
    /// A new window of the editor (a native alert).
    Window(Window),
}

/// Whether the consent's text names the fixture's interpreter, its script and
/// the scratch folder it runs in, as the Windows procedure requires of the
/// consent prompt (Confirm-FixtureCommand).
fn consent_names_fixture(text: &str, python: &str, scratch: &Path) -> bool {
    !python.is_empty()
        && text.contains(python)
        && text.contains("external_fixture.py")
        && text.contains(scratch.to_string_lossy().as_ref())
}

/// Run `command` (Run <external command>) through the palette and accept the
/// consent it must ask for, as Confirm-FixtureCommand does on Windows: find the
/// prompt, check that it names the fixture, answer Yes (the prompt's default is
/// No) and require it to close. No consent at all means the command cannot run
/// on this system; a consent that does not take Yes is the product's failure.
fn accept_external_consent(run: &mut Run, command: &str, python: &str) -> StepResult {
    let editor = run.pid()?;
    let windows = run.desktop.all_windows()?;
    let shown = run.stderr_events("event=prompt_shown")?.len();
    let answered = run.stderr_events("event=prompt_answered")?.len();
    let before = run.capture()?;
    run.command(command)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let consent = loop {
        if let Some(line) = run.stderr_events("event=prompt_shown")?.into_iter().nth(shown) {
            break Some(Consent::InApp(line));
        }
        let (window, _) = pick_dialog(&windows, run.desktop.all_windows()?, |window| {
            dialog_owner(window, editor)
        });
        if let Some(window) = window {
            break Some(Consent::Window(window));
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let image = run.shot("external consent")?;
    let rect = run.rect(Region::Center, &image);
    let center_diff = image.diff_fraction(&before, rect);
    let scratch = run.scratch.clone();
    match consent {
        None => {
            run.record(
                "external consent missing",
                serde_json::json!({"command": command, "center_diff": center_diff}),
            );
            Err(run
                .needs(
                    Service::ExternalProcesses,
                    format!("External command consent did not appear after {command}; the command never ran"),
                )
                .into())
        }
        Some(Consent::InApp(line)) => {
            let named = consent_names_fixture(&line, python, &scratch);
            if !named {
                run.gap(
                    "consent text names the fixture's python, external_fixture.py and the scratch folder",
                    GapKind::Unobservable(Service::Accessibility),
                    "the prompt's diagnostic line does not carry its full text; Windows reads it through UI Automation",
                );
            }
            // The in-app prompt takes a button's access key; Yes is 'Y'.
            run.key("Y")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while run.stderr_events("event=prompt_answered")?.len() <= answered {
                if Instant::now() >= deadline {
                    run.record("external consent not accepted", serde_json::json!({"prompt": line}));
                    return Err(Failure::product("External command consent did not take its Yes access key").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            run.record(
                "external explicit consent",
                serde_json::json!({"prompt": line, "names_fixture": named, "accepted_with": "Y", "center_diff": center_diff}),
            );
            Ok(())
        }
        Some(Consent::Window(window)) => {
            run.unobservable("consent text names the fixture's python, external_fixture.py and the scratch folder");
            // A native alert's Yes, by its mnemonic.
            run.desktop.key(&window, "Alt+Y")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while run.desktop.all_windows()?.iter().any(|known| known.id == window.id) {
                if Instant::now() >= deadline {
                    return Err(Failure::product("External command consent did not close after Yes").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            run.record(
                "external explicit consent",
                serde_json::json!({"window": window.json(), "accepted_with": "Alt+Y"}),
            );
            let main = run.window()?;
            run.desktop.focus(&main)?;
            Ok(())
        }
    }
}

/// Save once both panes have settled, and require `expected` on disk. The
/// Windows procedure observes both panes after each edit (Observe-Panes)
/// before its next action; the saved bytes stand in for that read here.
fn save_settled(run: &mut Run, saved: &Path, expected: &str, stage: &str) -> Result<(), Failure> {
    run.settle(Duration::from_secs(5))?;
    run.key("Primary+S")?;
    run.expect_file(saved, expected.as_bytes(), stage)
}

fn split_clone_sync(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('split_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let first = Run::fixture_text(&fixture, "first")?;
    let second = Run::fixture_text(&fixture, "second")?;
    let saved = run.scratch.join("split.txt");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    run.step(
        "s1",
        "Clone to Other View showed the same Unicode fixture in a second pane.",
        |run| {
            run.substituted("native Open", OPEN_ROUTE);
            run.launch(Launch::files(&[saved.as_path()]))?;
            let before = run.shot("split opened")?;
            run.command("Clone to Other View")?;
            run.expect_change(&before, Region::Right, 0.01, "split cloned")?;
            run.unobservable("two distinct pane providers with the exact text");
            Ok(())
        },
    );
    run.step(
        "s2",
        "An edit and one Undo from each pane changed the shared document exactly (each edit was saved and its Undo saved back); the fixture ends unchanged.",
        |run| {
            run.keys(&["Primary+End"])?;
            run.text("PRIMARY")?;
            save_settled(run, &saved, &first, "split primary edit")?;
            run.key("Primary+Z")?;
            save_settled(run, &saved, &initial, "split primary Undo")?;
            run.key("F6")?;
            run.key("Primary+End")?;
            run.text("SECONDARY")?;
            save_settled(run, &saved, &second, "split secondary edit")?;
            run.key("Primary+Z")?;
            save_settled(run, &saved, &initial, "split unchanged disk")?;
            run.unobservable("both panes' text after each edit and Undo (the saved bytes stand in)");
            Ok(())
        },
    );
    run.step(
        "s3",
        "With synchronized vertical scrolling, Page Down in one pane moved both panes; Close Split View returned to one pane.",
        |run| {
            run.command("Synchronize Vertical Scrolling")?;
            run.gap("sync menu item checked", GapKind::WindowsOnly, steps::NATIVE_MENU_STATE);
            run.key("F6")?;
            run.key("Primary+Home")?;
            let before = run.settle(Duration::from_secs(3))?;
            run.key("PageDown")?;
            run.expect_change(&before, Region::Left, 0.01, "split scroll left pane")?;
            let after = run.expect_change(&before, Region::Right, 0.01, "split scroll right pane")?;
            let _ = run.shot("split synchronized")?;
            run.unobservable("primary pane keeps focus and both first visible rows agree");
            run.command("Close Split View")?;
            run.expect_change(&after, Region::Right, 0.01, "split collapsed")?;
            run.expect_file(&saved, initial.as_bytes(), "split collapsed disk")?;
            Ok(())
        },
    );
    Ok(())
}

fn portable(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('portable_fixture').fixture('dark')")?;
    run.fixture = fixture["identity"].clone();
    let original = run.scratch.join("portable-original");
    let relocated = run.scratch.join("portable-relocated");
    let executable = original.join("bareline");
    std::fs::create_dir_all(&original).map_err(|error| Failure::harness(error.to_string()))?;
    std::fs::copy(&run.env.exe, &executable).map_err(|error| Failure::harness(error.to_string()))?;
    run.write(&original.join("bareline.portable"), b"")?;
    let settings = run.write_settings(&original.join("data"), "utf-8", "crlf")?;
    let document = run.scratch.join("portable-document.txt");
    run.write(&document, Run::fixture_text(&fixture, "document")?.as_bytes())?;
    let wanted = Run::fixture_text(&fixture, "settings")?;
    let installed = run.layout.installed_roots.clone();
    let mut document_tab = None;
    run.step(
        "s1",
        "The pinned editor ran from an isolated portable layout; editing its own settings wrote the exact defaults under data, with no installed-profile files.",
        |run| {
            run.substituted("native Open (settings and document)", OPEN_ROUTE);
            run.launch(Launch {
                executable: Some(&executable),
                session: true,
                files: &[document.as_path(), settings.as_path()],
            })?;
            run.key("Primary+A")?;
            for (index, line) in wanted.split('\n').enumerate() {
                if index > 0 {
                    run.key("Return")?;
                }
                if !line.is_empty() {
                    run.text(line)?;
                }
            }
            run.key("Primary+S")?;
            run.expect_file(&settings, wanted.as_bytes(), "portable settings edited")?;
            run.key("Primary+W")?;
            document_tab = Some(run.settle(Duration::from_secs(3))?);
            let _ = run.shot("portable document tab")?;
            run.unobservable("portable session source text");
            let files = run.profile_files(&installed);
            run.record("portable containment before", serde_json::json!({"external_profile_files": files}));
            if !files.is_empty() {
                return Err(Failure::product("Persistent state escaped portable data root").into());
            }
            Ok(())
        },
    );
    let document_tab = document_tab.unwrap_or_else(blank);

    run.step(
        "s2",
        "A clean Exit wrote the portable session; the whole owned package and state moved to a new folder with identical file identities.",
        |run| {
            run.exit()?;
            let before = inventory(&original)?;
            if !original.join("data/session.json").is_file() {
                // Without the instance handoff every window runs separately
                // and leaves the session alone.
                return Err(run
                    .needs(Service::SingleInstance, "Portable session file missing after a clean Exit".into())
                    .into());
            }
            std::fs::rename(&original, &relocated).map_err(|error| Failure::harness(error.to_string()))?;
            let after = inventory(&relocated)?;
            run.record(
                "portable relocation",
                serde_json::json!({"old_exists": original.exists(), "inventory_before": before, "inventory_after": after}),
            );
            if before != after {
                return Err(Failure::product("Portable relocation changed package/state bytes").into());
            }
            Ok(())
        },
    );
    run.step(
        "s3",
        "The relocated editor restored its session without installation; a new document saved with the persisted UTF-16BE default, and all profile state stayed portable.",
        |run| {
            run.launch(Launch {
                executable: Some(&relocated.join("bareline")),
                session: true,
                files: &[],
            })?;
            // As in the regression procedure: a blank Untitled may open beside
            // the restored tab before the restore begins reading.
            run.expect_same(&document_tab, Region::FirstTab, 0.0002, "portable restored session tab")?;
            run.unobservable("restored session text");
            let files = run.profile_files(&installed);
            run.record("portable containment after", serde_json::json!({"external_profile_files": files}));
            if !files.is_empty() {
                return Err(Failure::product("Persistent state escaped portable data root").into());
            }
            run.retain(&relocated.join("data/session.json"), "session.json");
            run.key("Primary+N")?;
            run.text(&Run::fixture_text(&fixture, "new_text")?)?;
            let target = run.scratch.join("relocated-new.txt");
            run.choose_in_dialog(
                Opener::Key {
                    chord: "Primary+Shift+S",
                    label: "Save As",
                },
                &target,
            )?;
            let text = Run::fixture_text(&fixture, "new_text")?;
            let mut expected = vec![0xFE, 0xFF];
            expected.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            run.expect_file(&target, &expected, "portable relocated encoding")?;
            Ok(())
        },
    );
    Ok(())
}

/// Close the previous step's editor: a clean Exit after a passing step, a kill
/// otherwise (Regression-Stop). A killed editor leaves recovery journals of its
/// unsaved documents, which the next launch would restore into the next step;
/// they are removed so every step starts from its own fixture.
fn stop_regression_editor(run: &mut Run) {
    if run.editor.is_none() {
        return;
    }
    let previous_failed = run
        .steps
        .last()
        .is_some_and(|step| matches!(step.status, StepStatus::Fail(_)));
    let closed = !previous_failed
        && match run.exit() {
            Ok(()) => true,
            Err(failure) => {
                run.record(
                    "regression close failure",
                    serde_json::json!({"detail": failure.detail}),
                );
                false
            }
        };
    if !closed {
        run.kill();
        let recovery = run.layout.profile.join("recovery");
        let removed = std::fs::remove_dir_all(&recovery).is_ok();
        run.record(
            "regression editor killed",
            serde_json::json!({"recovery": recovery, "recovery_removed": removed}),
        );
    }
}

/// Each regression step starts its own owned editor (Regression-Fresh).
fn fresh(run: &mut Run, files: &[&Path], session: bool) -> Result<(), Failure> {
    stop_regression_editor(run);
    run.launch(Launch {
        executable: None,
        session,
        files,
    })
}

fn ui_regressions(run: &mut Run) -> Result<(), Failure> {
    let fixture = run.python("__import__('regressions_fixture').fixture()")?;
    run.fixture = fixture["identity"].clone();
    let initial = Run::fixture_text(&fixture, "initial")?;
    let saved = run.scratch.join("regression.txt");
    let busy = run.scratch.join("busy-document.txt");
    run.write(&saved, initial.as_bytes())?;
    run.write_settings(&run.layout.profile.clone(), "utf-8", "crlf")?;
    let profile = run.layout.profile.clone();
    run.independent_step(
        "s1",
        "ISSUE-005: Ctrl+F took the typed query without changing the document; after Escape a marker typed at Ctrl+Home landed in the Editor, so focus returned there.",
        |run| {
            fresh(run, &[saved.as_path()], false)?;
            let before = run.shot("find source opened")?;
            run.key("Primary+F")?;
            run.text(&Run::fixture_text(&fixture, "query")?)?;
            run.expect_change(&before, Region::Body, 0.002, "find panel shows the query")?;
            run.unobservable("Find field owns UIA keyboard focus with the typed value; announced match count");
            run.key("Escape")?;
            run.key("Primary+Home")?;
            run.text("Q")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, format!("Q{initial}").as_bytes(), "find escape editor focus")?;
            run.key("Primary+Z")?;
            run.key("Primary+S")?;
            run.expect_file(&saved, initial.as_bytes(), "find source restored")?;
            Ok(())
        },
    );
    run.independent_step(
        "s2",
        "ISSUE-008: Ctrl+W on a dirty Untitled showed an owned save prompt; Cancel kept the text, Don't Save discarded it, and the window stayed usable.",
        |run| {
            fresh(run, &[], false)?;
            let clean = run.shot("untitled empty")?;
            run.text(&Run::fixture_text(&fixture, "untitled")?)?;
            run.expect_dirty(&clean, true, "untitled dirty")?;
            let before = run.shot("untitled typed")?;
            let editor = run.pid()?;
            let windows = run.desktop.all_windows()?;
            run.key("Primary+W")?;
            std::thread::sleep(Duration::from_secs(2));
            let (prompt, _) = pick_dialog(&windows, run.desktop.all_windows()?, |window| {
                dialog_owner(window, editor)
            });
            let owned = prompt.is_some();
            let modal = run.capture().map(|image| {
                let rect = run.rect(Region::Center, &image);
                image.diff_fraction(&before, rect)
            })?;
            let _ = run.shot("close prompt")?;
            run.record("close prompt cancel", serde_json::json!({"new_window": owned, "center_diff": modal}));
            if !owned && modal < 0.02 {
                return Err(run
                    .needs(Service::Dialogs, "Save prompt did not appear for Ctrl+W on a dirty Untitled".into())
                    .into());
            }
            run.key("Escape")?;
            run.expect_dirty(&clean, true, "untitled still dirty")?;
            let dirty = run.capture()?;
            run.key("Primary+W")?;
            std::thread::sleep(Duration::from_secs(1));
            // Alt+N is the Don't Save mnemonic.
            run.key("Alt+N")?;
            // Closing the last tab leaves a fresh, clean Untitled whose number
            // differs from the first one's: the typed text is gone from the
            // body and the modified tab is gone from the strip.
            run.expect_same(&clean, Region::Body, 0.0005, "untitled discarded body")?;
            run.expect_change(&dirty, Region::Tabs, 0.0002, "untitled discarded tab")?;
            run.unobservable("the remaining tab is a clean Untitled document (its name and modified state)");
            run.gap("window and every top-level menu enabled", GapKind::WindowsOnly, steps::NATIVE_MENU_STATE);
            Ok(())
        },
    );
    run.independent_step(
        "s3",
        "U08: after Clone and each F6 typing landed at the focused pane's own caret: 'A' at one pane's start and 'Z' at the other's end of the shared text.",
        |run| {
            // Each step starts from its own fixture, whatever an earlier one left.
            stop_regression_editor(run);
            run.write(&saved, initial.as_bytes())?;
            fresh(run, &[saved.as_path()], false)?;
            run.command("Clone to Other View")?;
            run.keys(&["Primary+End", "F6", "Primary+Home", "F6", "F6"])?;
            run.text("A")?;
            run.key("F6")?;
            run.text("Z")?;
            run.key("Primary+S")?;
            run.expect_file(
                &saved,
                Run::fixture_text(&fixture, "split_edited")?.as_bytes(),
                "pane typing at each caret",
            )?;
            run.keys(&["Primary+Z", "Primary+Z", "Primary+S"])?;
            run.expect_file(&saved, initial.as_bytes(), "split edits undone")?;
            run.command("Close Split View")?;
            run.unobservable("exactly one pane provider has UIA keyboard focus after each F6");
            Ok(())
        },
    );
    run.independent_step(
        "s4",
        "PR-T05: Ctrl+W during a running paged save was deferred as busy and completed once, without a prompt, after the save; the saved bytes include the edit.",
        |run| {
            if !busy.exists() {
                run.python(&format!(
                    "__import__('regressions_fixture').generate_busy(__import__('pathlib').Path({:?}))",
                    busy.to_string_lossy()
                ))?;
            }
            stop_regression_editor(run);
            let loaded = run.loaded_tab(&busy)?;
            fresh(run, &[busy.as_path()], false)?;
            run.wait_loaded(&loaded, Duration::from_secs(60))?;
            run.key("Primary+Home")?;
            let clean = run.settle(Duration::from_secs(10))?;
            let edit = Run::fixture_text(&fixture, "busy_edit")?;
            run.text(&edit)?;
            // As on Windows, Save follows the observed edit and modified tab.
            run.expect_change(&clean, Region::Body, 0.0003, "busy edited viewport")?;
            run.expect_dirty(&clean, true, "busy dirty")?;
            let before = run.settle(Duration::from_secs(10))?;
            let _ = run.shot("busy edited")?;
            run.key("Primary+S")?;
            run.key("Primary+W")?;
            let trace = run.evidence.join(format!("command-trace-{}.jsonl", run.launches));
            let deadline = Instant::now() + Duration::from_secs(90);
            let rows = loop {
                let rows: Vec<serde_json::Value> = std::fs::read_to_string(&trace)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                let image = run.capture()?;
                let rect = run.rect(Region::Tabs, &image);
                let closed = image.diff_fraction(&before, rect) >= 0.004;
                if closed || Instant::now() >= deadline {
                    break rows;
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            let _ = run.shot("busy close completed")?;
            run.record("busy close trace", serde_json::json!({"records": rows}));
            let stage = |stage: &str, detail: &str| {
                rows.iter()
                    .any(|row| row["event"] == "qa_close_command" && row["stage"] == stage && row["detail"] == detail)
            };
            if !stage("queued", "document") {
                return Err(Failure::product(
                    "Close command trace missing: Ctrl+W after Ctrl+S on the edited paged document never queued a close",
                )
                .into());
            }
            if !stage("deferred", "document-busy") {
                return Err(Failure::harness("Busy precondition not established: the save completed before Close").into());
            }
            let line = Run::fixture_text(&fixture, "busy_line")?;
            let size = fixture["busy_bytes"].as_u64().unwrap_or(0) + edit.len() as u64;
            let prefix = {
                use std::io::Read as _;
                let mut buffer = vec![0u8; 256];
                let count = std::fs::File::open(&busy)
                    .and_then(|mut file| file.read(&mut buffer))
                    .map_err(|error| Failure::harness(error.to_string()))?;
                String::from_utf8_lossy(&buffer[..count]).split_inclusive('\n').next().unwrap_or("").to_owned()
            };
            let bytes = std::fs::metadata(&busy).map(|metadata| metadata.len()).unwrap_or(0);
            run.record("busy saved prefix", serde_json::json!({"prefix": prefix, "bytes": bytes}));
            if prefix != format!("{edit}{line}") || bytes != size {
                return Err(Failure::product("Busy save bytes differ").into());
            }
            run.unobservable("busy document tab absent (the tab strip change stands in)");
            Ok(())
        },
    );
    run.independent_step(
        "s5",
        "ISSUE-030: after a clean Exit with session restore enabled, each relaunch showed a visible window with a first frame and restored the document tab.",
        |run| {
            stop_regression_editor(run);
            run.write(&saved, initial.as_bytes())?;
            let session = profile.join("session.json");
            let _ = std::fs::remove_file(&session);
            run.substituted("native Open", OPEN_ROUTE);
            fresh(run, &[saved.as_path()], true)?;
            let opened = run.settle(Duration::from_secs(3))?;
            run.exit()?;
            if !session.is_file() {
                return Err(run
                    .needs(Service::SingleInstance, "Session was not saved on a clean Exit".into())
                    .into());
            }
            run.retain(&session, "session.json");
            let relaunches = fixture["relaunches"].as_u64().unwrap_or(2);
            for launch in 1..=relaunches {
                if launch > 1 {
                    run.exit()?;
                }
                // The launch requires a visible, mapped window and a first frame.
                run.launch(Launch {
                    executable: None,
                    session: true,
                    files: &[],
                })?;
                // The restored document's tab leads the strip. A launch without
                // files may also open a blank Untitled before the restore begins
                // reading (shell/startup.rs pins it), so only the first tab is
                // compared; Windows checks the active document's text.
                run.expect_same(&opened, Region::FirstTab, 0.0002, &format!("relaunch {launch} restored document tab"))?;
                run.unobservable("restored document text and that it is the active document");
            }
            Ok(())
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, pid: u32) -> Window {
        Window {
            id: id.into(),
            pid: Some(pid),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            title: None,
        }
    }

    #[test]
    fn only_a_new_window_of_the_editor_or_a_chooser_host_is_taken_for_the_dialog() {
        // A terminal and the editor were on the desktop before the opener ran.
        let before = [window("terminal", 50), window("editor", 100)];
        let owner = |window: &Window| window.pid == Some(100) || window.pid == Some(200);
        // Nothing new: no dialog, and the terminal is never a candidate.
        let (dialog, foreign) = pick_dialog(&before, before.to_vec(), owner);
        assert_eq!((dialog, foreign), (None, Vec::new()));
        // A new window of another application appears first: refused.
        let now = vec![
            window("terminal", 50),
            window("editor", 100),
            window("notification", 60),
        ];
        let (dialog, foreign) = pick_dialog(&before, now, owner);
        assert_eq!(dialog, None);
        assert_eq!(foreign, [window("notification", 60)]);
        // The portal's chooser appears: taken, the foreign window still refused.
        let now = vec![
            window("terminal", 50),
            window("notification", 60),
            window("chooser", 200),
            window("editor", 100),
        ];
        let (dialog, foreign) = pick_dialog(&before, now, owner);
        assert_eq!(dialog, Some(window("chooser", 200)));
        assert_eq!(foreign, [window("notification", 60)]);
    }

    #[test]
    fn chooser_hosts_are_the_portal_backends_and_the_macos_panel_service() {
        // Linux truncates comm to 15 bytes.
        assert!(is_chooser_host("xdg-desktop-por"));
        assert!(is_chooser_host("xdg-desktop-portal-gtk"));
        assert!(is_chooser_host(
            "/System/Library/Frameworks/AppKit.framework/XPCServices/OpenAndSave.xpc/Contents/MacOS/\
             com.apple.appkit.xpc.openAndSavePanelService"
        ));
        for other in [
            "bash",
            "Terminal",
            "/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            "Finder",
            "",
        ] {
            assert!(!is_chooser_host(other), "{other}");
        }
        let unowned = Window {
            pid: None,
            ..window("xt", 1)
        };
        assert!(
            !dialog_owner(&unowned, 1),
            "a window without an owner never takes input"
        );
        assert!(dialog_owner(&window("editor", std::process::id()), std::process::id()));
    }

    #[test]
    fn the_consent_must_name_the_fixture_interpreter_script_and_folder() {
        let scratch = Path::new("/work/target/journey/macro_external-1-2");
        let python = "/usr/bin/python3";
        let line = "event=prompt_shown title=\"Bareline - Run External Command\" \
                    instruction=\"Run this direct executable command?\n/usr/bin/python3 \
                    /work/tests/e2e/external_fixture.py /work/target/journey/macro_external-1-2\" buttons=[\"Yes\", \"No\"]";
        assert!(consent_names_fixture(line, python, scratch));
        assert!(!consent_names_fixture(line, "/opt/python3.12", scratch));
        assert!(!consent_names_fixture(
            &line.replace("external_fixture.py", "other.py"),
            python,
            scratch
        ));
        assert!(
            !consent_names_fixture(line, "", scratch),
            "an unknown interpreter never matches"
        );
    }

    /// The editor binds its instance socket at
    /// `<runtime>/bareline/instance-<24 hex digits>.sock`; a path longer than
    /// a Unix socket address cannot bind, and the window then runs
    /// independently and never saves the session.
    #[test]
    fn the_instance_socket_of_a_deep_scratch_home_fits_a_unix_socket_address() {
        let home = Path::new(
            "/home/runner/work/Bareline-Editor/Bareline-Editor/target/journey/ui_regressions-123456-1791337033423832192",
        );
        let layout = Layout::new(home);
        let runtime = layout
            .environment()
            .into_iter()
            .find_map(|(name, path)| (name == "XDG_RUNTIME_DIR").then_some(path))
            .unwrap();
        let socket = runtime
            .join("bareline")
            .join(format!("instance-{}.sock", "0".repeat(24)));
        // macOS allows 104 bytes including the terminating NUL.
        assert!(socket.as_os_str().len() < 104, "{}", socket.display());
        assert!(
            !runtime.starts_with(home),
            "the runtime folder must not live in the scratch home"
        );
        assert_ne!(
            runtime,
            Layout::new(&home.join("other")).runtime,
            "each attempt has its own"
        );
    }

    #[test]
    fn only_the_display_refusing_the_connection_is_retried() {
        let refused = "event=startup_failed error=os error at winit-0.30.13/src/platform_impl/linux/mod.rs:788: \
                       Failed to open connection to X server";
        assert_eq!(refused_by_display(refused), cfg!(target_os = "linux"));
        assert!(!refused_by_display("event=startup_failed error=renderer unavailable"));
    }

    #[test]
    fn a_black_or_empty_capture_is_not_a_painted_window() {
        let image = |pixels: Vec<u8>| Image {
            width: i32::try_from(pixels.len() / 4).unwrap(),
            height: 1,
            pixels,
        };
        assert!(!painted(&image(Vec::new())));
        assert!(!painted(&image(vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255])));
        // Alpha alone does not count; one differing channel does.
        assert!(!painted(&image(vec![30, 30, 30, 255, 30, 30, 30, 0])));
        assert!(painted(&image(vec![30, 30, 30, 255, 30, 31, 30, 255])));
    }

    #[test]
    fn a_tool_that_does_not_answer_is_killed_at_its_deadline() {
        let started = Instant::now();
        let failure = output_within(Command::new("sleep").arg("30"), "sleep", Duration::from_millis(200)).unwrap_err();
        assert_eq!(failure.class.name(), "timeout");
        assert!(started.elapsed() < Duration::from_secs(10));
        let missing = output_within(
            &mut Command::new("bareline-no-such-tool"),
            "probe",
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(missing.class.name(), "environment");
        let output = output_within(
            Command::new("sh").args(["-c", "echo ok"]),
            "sh",
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
    }
}
