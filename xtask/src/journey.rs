// SPDX-License-Identifier: MPL-2.0
//! Native journey runner: launches the built editor in an isolated data
//! directory, drives it through the platform's input channel, captures window
//! screenshots, and asserts on pixel regions, saved bytes and the diagnostics.
//!
//! Each journey is small and independent: it launches its own process into a
//! throwaway data root, performs a few steps, and is torn down by killing the
//! process it started (never any other editor instance).
//!
//! This file is the platform-neutral driver: the registry type, argument
//! handling, run identity, failure classification and the evidence files. The
//! platform halves live beside it. `windows.rs` drives Win32 with `SendInput`,
//! native menu commands, BitBlt captures and UI Automation. On Linux and macOS
//! `ordinary.rs` runs the ordinary journeys of tests/e2e/journeys.json through
//! the window, input and screenshot tools of `linux.rs` (X11: xdotool and
//! ImageMagick under Xvfb) or `macos.rs` (Quartz, System Events, screencapture).
//! `steps.rs` shapes their step results and the cross-attempt summary.

use std::cell::{Cell, RefCell};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod ordinary;
mod steps;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use ordinary::journeys;
#[cfg(windows)]
use windows::journeys;

// ------------------------------------------------------------------ registry --

/// One encoded journey. `run` returns `Ok(())` on pass or `Err(reason)` on fail.
struct Journey {
    name: &'static str,
    summary: &'static str,
    run: fn(&Env) -> Result<(), String>,
}

// ------------------------------------------------------------------- driver ---

const USAGE: &str = "Usage: cargo xtask journey <name|ordinary|all> [--build] [--retry-of=<run-id>] \
                     [--executable=<path>] [--output=<dir>] [--attempts=<1..10>] [--commit=<sha>] \
                     [--summary=<markdown file>] [--no-fail]\n       \
                     cargo xtask journey summarize --results=<dir> [--output=<dir>] [--summary=<markdown file>]";

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.first().is_some_and(|arg| arg == "summarize") {
        return steps::summarize_command(&args[1..]);
    }
    let build = args.iter().any(|a| a == "--build");
    let no_fail = args.iter().any(|a| a == "--no-fail");
    let option = |name: &str| args.iter().find_map(|arg| arg.strip_prefix(name));
    let retry_of = option("--retry-of=");
    let commit = option("--commit=");
    let summary = option("--summary=").map(PathBuf::from);
    let attempts = option("--attempts=")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid attempt count {value:?}"))
        })
        .transpose()?
        .unwrap_or(1);
    if !(1..=10).contains(&attempts) {
        return Err("Attempt count must be 1..10".into());
    }
    let selectors: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();
    let name = *selectors.first().ok_or(USAGE)?;

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

    let exe = match option("--executable=") {
        Some(path) => std::path::absolute(path)?,
        None => root.join(format!("target/debug/bareline{}", std::env::consts::EXE_SUFFIX)),
    };
    if !exe.exists() {
        return Err(format!(
            "{} not found; build it first (cargo build -p bareline) or pass --build",
            exe.display()
        )
        .into());
    }
    let output = match option("--output=") {
        Some(path) => std::path::absolute(path)?,
        None => root.join("target/journey/results"),
    };

    let all = journeys();
    let selected: Vec<&Journey> = match name {
        "all" => all.iter().collect(),
        "ordinary" => all.iter().filter(|j| steps::ORDINARY.contains(&j.name)).collect(),
        _ => all.iter().filter(|j| j.name == name).collect(),
    };
    if selected.is_empty() {
        let names: Vec<&str> = all.iter().map(|j| j.name).collect();
        let hint = if steps::ORDINARY.contains(&name) || name == "ordinary" {
            " (on Windows the ordinary journeys run through tests/e2e/journey_matrix.py)"
        } else {
            ""
        };
        return Err(format!("unknown journey '{name}'{hint}; known: {}", names.join(", ")).into());
    }

    let run_identity = run_identity(&root, &exe, retry_of, commit)?;
    let run_id = run_identity["run_id"].as_str().ok_or("run id unavailable")?.to_owned();
    let evidence_path = output.join(format!("{run_id}.json"));
    eprintln!("journey: run_identity={run_identity}");
    let env = Env {
        exe,
        root,
        run_id: run_id.clone(),
        output,
        attempt: Cell::new(1),
        report: RefCell::new(None),
    };
    let total = selected.len() * attempts;
    let mut failures = Vec::new();
    let mut results = Vec::new();
    persist_run_evidence(&evidence_path, &run_identity, total, &results)?;
    for attempt in 1..=attempts {
        env.attempt.set(attempt);
        for journey in &selected {
            let label = if attempts > 1 {
                format!(" (attempt {attempt} of {attempts})")
            } else {
                String::new()
            };
            eprintln!("journey {} â€” {} â€¦{label}", journey.name, journey.summary);
            let outcome = (journey.run)(&env);
            if let Some(report) = env.report.borrow_mut().take() {
                // A step journey reports every step and its own classification.
                let line = steps::outcome_line(&report);
                println!("{line}");
                if report["status"] == "failed" {
                    failures.push(journey.name);
                }
                results.push(report);
            } else {
                match outcome {
                    Ok(()) => {
                        println!("PASS {}", journey.name);
                        results.push(serde_json::json!({"name": journey.name, "status": "passed"}));
                    }
                    Err(reason) => {
                        println!("FAIL {} â€” {reason}", journey.name);
                        failures.push(journey.name);
                        results.push(serde_json::json!({
                            "name": journey.name,
                            "status": "failed",
                            "classification": classify_failure(&reason),
                            "detail": reason,
                        }));
                    }
                }
            }
            persist_run_evidence(&evidence_path, &run_identity, total, &results)?;
        }
    }

    let passed = results.iter().filter(|result| result["status"] == "passed").count();
    eprintln!(
        "journey: top_level_passed={passed} top_level_failed={} top_level_total={total}",
        failures.len(),
    );
    eprintln!("journey: evidence={}", evidence_path.display());
    eprintln!("journey: evidence_sha256={}", hash_file(&evidence_path)?);
    if results.iter().any(|result| result.get("steps").is_some()) {
        let summary_path = steps::write_summary(&env.output, &results, summary.as_deref())?;
        eprintln!("journey: summary={}", summary_path.display());
    }
    if failures.is_empty() || no_fail {
        Ok(())
    } else {
        Err(format!("{} journey(s) failed: {}", failures.len(), failures.join(", ")).into())
    }
}

fn run_identity(
    root: &Path,
    exe: &Path,
    retry_of: Option<&str>,
    commit: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let run_id = format!("{}-{stamp}", std::process::id());
    let (head, working_tree_dirty, source_manifest_sha256, source_identity_error) = match source_identity(root) {
        Ok((head, dirty, manifest)) => (head, Some(dirty), Some(manifest), None),
        // A copy of the sources without git metadata (the WSL helper syncs the
        // worktree without .git) can still name the commit it was taken from.
        Err(error) => match commit {
            Some(commit) => (commit.to_owned(), None, None, Some(error.to_string())),
            None => return Err(error),
        },
    };
    let mut identity = serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "retry_of": retry_of,
        "head": head,
        "working_tree_dirty": working_tree_dirty,
        "source_manifest_sha256": source_manifest_sha256,
        "executable": exe,
        "executable_sha256": hash_file(exe)?,
    });
    if let Some(error) = source_identity_error {
        identity["source_identity_error"] = serde_json::json!(error);
    }
    if let Some(commit) = commit
        && commit != head
    {
        return Err(format!("--commit={commit} differs from the checked-out HEAD {head}").into());
    }
    Ok(identity)
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
    let skipped = results.iter().filter(|result| result["status"] == "skipped").count();
    let document = serde_json::json!({
        "identity": identity,
        "top_level_total": top_level_total,
        "top_level_completed": results.len(),
        "top_level_passed": passed,
        "top_level_failed": failed,
        "top_level_skipped": skipped,
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
    /// Where each attempt's evidence directory, the run evidence and the
    /// summary are written.
    output: PathBuf,
    /// The attempt being run (1-based).
    attempt: Cell<usize>,
    /// The step report a step journey leaves for the driver.
    report: RefCell<Option<serde_json::Value>>,
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

// -------------------------------------------------------------------- pixels --

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
        // Windows images name the interpreter python; Linux and macOS python3.
        let output = Command::new(if cfg!(windows) { "python" } else { "python3" })
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
        assert_eq!(evidence["top_level_skipped"], 0);
        std::fs::remove_file(path).unwrap();
    }
}
