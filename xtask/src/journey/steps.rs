// SPDX-License-Identifier: MPL-2.0
//! Step results of the ordinary journeys on Linux and macOS, and the summary
//! across journeys and attempts.
//!
//! A step passes, fails with one failure class, is skipped with a recorded
//! reason, or is not run because an earlier step failed. A passing step can
//! still name gaps: checks it could not make on this platform (for example a
//! UI Automation text read while no accessibility tree is published), which
//! are listed rather than silently dropped. A failure caused by a service the
//! port has as a library but has not wired into the shell yet is classified
//! `service_not_wired` with the service's name, so the issue list says what to
//! re-run once that service lands.
#![cfg_attr(
    windows,
    allow(
        dead_code,
        reason = "Windows runs the ordinary journeys through tests/e2e/journey_matrix.py; it compiles the step \
                  results for the summary command and the unit tests"
    )
)]

use std::path::{Path, PathBuf};

/// The ordinary tier of tests/e2e/journeys.json, in manifest order (checked by
/// a unit test against the manifest).
pub(super) const ORDINARY: [&str; 11] = [
    "plain_text",
    "code_config",
    "regex_transform",
    "column_multi_cursor",
    "huge_log_tail",
    "workspace",
    "udl",
    "macro_external",
    "split_clone_sync",
    "portable",
    "ui_regressions",
];

/// A platform service the journeys need that the port provides as a library
/// but may not have wired into the shell yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Service {
    /// File and folder choosers, save and message prompts (the XDG portal and
    /// in-app prompts on Linux, AppKit panels on macOS).
    Dialogs,
    FileWatching,
    Clipboard,
    SingleInstance,
    ExtensionHost,
    /// The accessibility tree (AT-SPI on Linux, NSAccessibility on macOS) the
    /// Windows journeys read through UI Automation.
    Accessibility,
    /// The native menu bar (macOS); Linux reaches commands through the palette.
    Menus,
}

impl Service {
    pub(super) const ALL: [Service; 7] = [
        Service::Dialogs,
        Service::FileWatching,
        Service::Clipboard,
        Service::SingleInstance,
        Service::ExtensionHost,
        Service::Accessibility,
        Service::Menus,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Service::Dialogs => "dialogs",
            Service::FileWatching => "file watching",
            Service::Clipboard => "clipboard",
            Service::SingleInstance => "single instance",
            Service::ExtensionHost => "extension host",
            Service::Accessibility => "accessibility",
            Service::Menus => "menus",
        }
    }
}

/// Why a step failed, most specific first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Class {
    /// The step needs a service that is not wired into the shell yet.
    ServiceNotWired(Service),
    /// The session could not provide the requested cell: no display, input
    /// refused by the system, a missing tool.
    Environment,
    /// A harness deadline expired before an expected state was observed.
    Timeout,
    /// The harness itself failed (spawn, fixture, capture).
    Harness,
    /// Product behaviour differs from the step's oracle.
    Product,
}

impl Class {
    pub(super) fn name(self) -> &'static str {
        match self {
            Class::ServiceNotWired(_) => "service_not_wired",
            Class::Environment => "environment",
            Class::Timeout => "timeout",
            Class::Harness => "harness",
            Class::Product => "product",
        }
    }

    fn label(self) -> String {
        match self {
            Class::ServiceNotWired(service) => format!("service not wired ({})", service.name()),
            Class::Environment => "environment".into(),
            Class::Timeout => "timeout".into(),
            Class::Harness => "harness".into(),
            Class::Product => "product".into(),
        }
    }

    fn service(self) -> Option<Service> {
        match self {
            Class::ServiceNotWired(service) => Some(service),
            _ => None,
        }
    }
}

/// A classified step failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Failure {
    pub(super) class: Class,
    pub(super) detail: String,
}

impl Failure {
    pub(super) fn new(class: Class, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
        }
    }
    pub(super) fn product(detail: impl Into<String>) -> Self {
        Self::new(Class::Product, detail)
    }
    pub(super) fn harness(detail: impl Into<String>) -> Self {
        Self::new(Class::Harness, detail)
    }
    pub(super) fn timeout(detail: impl Into<String>) -> Self {
        Self::new(Class::Timeout, detail)
    }
    pub(super) fn environment(detail: impl Into<String>) -> Self {
        Self::new(Class::Environment, detail)
    }
    pub(super) fn not_wired(service: Service, detail: impl Into<String>) -> Self {
        Self::new(Class::ServiceNotWired(service), detail)
    }
}

/// Why a check inside a passing step was not made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GapKind {
    /// The check needs a service this platform's shell does not provide yet.
    Unobservable(Service),
    /// The checked surface exists only on Windows.
    WindowsOnly,
    /// The Windows route needs a native dialog; an equivalent command-line or
    /// session route ran instead.
    Substituted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Gap {
    pub(super) check: String,
    pub(super) kind: GapKind,
    pub(super) note: String,
}

/// The Windows-only surfaces some ordinary journeys check, with the reason the
/// check is skipped elsewhere.
pub(super) const NATIVE_MENU_STATE: &str = "Win32 menu-bar state (enabled and checked items) has no Linux counterpart: \
     the shell draws no menu bar there and commands run through the command palette";
pub(super) const WIN32_PROCESS_TREE: &str =
    "Win32 process identities; the parent chain is read from /proc or ps instead";

/// The Linux and macOS shell seam, one file per service.
const SEAM: &str = "apps/bareline/src/shell/native/unix";

/// For each service, the seam file and a phrase only its stand-in contains.
/// The journeys run the editor built from the same checkout, so a failing
/// step that needs a service whose stand-in is still in place is classified
/// `service_not_wired`; once the service is wired the phrase is gone and the
/// same failure is a product failure.
const STAND_INS: [(Service, &str, &str); 7] = [
    (
        Service::Dialogs,
        "platform.rs",
        "SavePromptOutcome::Failure(NOT_IMPLEMENTED)",
    ),
    (
        Service::FileWatching,
        "watch.rs",
        "unsupported_io(Capability::FileWatch)",
    ),
    (Service::Clipboard, "platform.rs", "does not support the clipboard yet"),
    (
        Service::SingleInstance,
        "instance.rs",
        "does not support handing files to a running Bareline window",
    ),
    (
        Service::ExtensionHost,
        "extension_transport.rs",
        "does not support running extensions yet",
    ),
    (Service::Accessibility, "accessibility.rs", "no_platform_adapter"),
    (Service::Menus, "platform.rs", "fn command_id(&self, _menu_id: usize)"),
];

/// `Some(true)` while the seam still holds `service`'s stand-in, `Some(false)`
/// once it is gone, `None` when the seam cannot be read.
pub(super) fn seam_stand_in(root: &Path, service: Service) -> Option<bool> {
    let (_, file, phrase) = STAND_INS.iter().find(|(known, _, _)| *known == service)?;
    let text = std::fs::read_to_string(root.join(SEAM).join(file)).ok()?;
    Some(text.contains(phrase))
}

/// A step's terminal state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StepStatus {
    Pass,
    Fail(Failure),
    Skipped(String),
    NotRun(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StepRecord {
    pub(super) id: String,
    pub(super) status: StepStatus,
    /// What a passing step observed (the Windows procedures' wording).
    pub(super) observed: String,
    pub(super) gaps: Vec<Gap>,
}

impl StepRecord {
    fn json(&self) -> serde_json::Value {
        let (status, observed, class, service) = match &self.status {
            StepStatus::Pass => ("PASS", self.observed.clone(), None, None),
            StepStatus::Fail(failure) => (
                "FAIL",
                format!("{}: {}", failure.class.label(), failure.detail),
                Some(failure.class.name()),
                failure.class.service().map(Service::name),
            ),
            StepStatus::Skipped(reason) => ("SKIPPED", reason.clone(), None, None),
            StepStatus::NotRun(reason) => ("NOT_RUN", reason.clone(), None, None),
        };
        let gaps: Vec<_> = self
            .gaps
            .iter()
            .map(|gap| {
                let (kind, service) = match gap.kind {
                    GapKind::Unobservable(service) => ("unobservable", Some(service.name())),
                    GapKind::WindowsOnly => ("windows_only", None),
                    GapKind::Substituted => ("substituted", None),
                };
                serde_json::json!({"check": gap.check, "kind": kind, "service": service, "note": gap.note})
            })
            .collect();
        serde_json::json!({
            "id": self.id,
            "status": status,
            "observed": observed,
            "class": class,
            "service": service,
            "gaps": gaps,
        })
    }
}

/// The report one attempt of one journey leaves: `result.json` in its evidence
/// directory and the entry in the run evidence.
pub(super) fn report(
    journey: &str,
    attempt: usize,
    directory: &Path,
    platform: serde_json::Value,
    steps: &[StepRecord],
) -> serde_json::Value {
    let failed = steps.iter().find_map(|step| match &step.status {
        StepStatus::Fail(failure) => Some((step.id.as_str(), failure)),
        _ => None,
    });
    let passed = steps.iter().any(|step| step.status == StepStatus::Pass);
    let status = if failed.is_some() {
        "failed"
    } else if passed {
        "passed"
    } else {
        "skipped"
    };
    let skipped = steps.iter().find_map(|step| match &step.status {
        StepStatus::Skipped(reason) => Some(format!("{}: {reason}", step.id)),
        _ => None,
    });
    serde_json::json!({
        "schema_version": 1,
        "kind": "port-journey-result",
        "name": journey,
        "attempt": attempt,
        "status": status,
        "classification": failed.map(|(_, failure)| failure.class.name()),
        "service": failed.and_then(|(_, failure)| failure.class.service().map(Service::name)),
        "failing_step": failed.map(|(id, _)| id),
        "detail": failed.map(|(_, failure)| failure.detail.clone()).or(skipped),
        "directory": directory,
        "platform": platform,
        "steps": steps.iter().map(StepRecord::json).collect::<Vec<_>>(),
    })
}

/// One console line for a report: `PASS name`, `FAIL name — s2 ...`, `SKIP name — ...`.
pub(super) fn outcome_line(report: &serde_json::Value) -> String {
    let name = report["name"].as_str().unwrap_or("?");
    match report["status"].as_str() {
        Some("passed") => format!("PASS {name}"),
        Some("failed") => format!(
            "FAIL {name} — {} {}{}: {}",
            report["failing_step"].as_str().unwrap_or("?"),
            report["classification"].as_str().unwrap_or("?"),
            report["service"]
                .as_str()
                .map(|service| format!(" ({service})"))
                .unwrap_or_default(),
            report["detail"].as_str().unwrap_or("")
        ),
        _ => format!("SKIP {name} — {}", report["detail"].as_str().unwrap_or("")),
    }
}

// ------------------------------------------------------------------- summary --

/// Combine reports (any order) into `summary.json` and `summary.md` in
/// `output`, append the Markdown to `markdown` when given (a GitHub step
/// summary) and return the JSON path.
pub(super) fn write_summary(
    output: &Path,
    reports: &[serde_json::Value],
    markdown: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let summary = summarize(reports);
    std::fs::create_dir_all(output)?;
    let path = output.join("summary.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&summary)?)?;
    let text = summary_markdown(&summary);
    std::fs::write(output.join("summary.md"), &text)?;
    if let Some(markdown) = markdown {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(markdown)?;
        file.write_all(text.as_bytes())?;
    }
    print!("{text}");
    Ok(path)
}

fn summarize(reports: &[serde_json::Value]) -> serde_json::Value {
    let mut names: Vec<&str> = reports.iter().filter_map(|report| report["name"].as_str()).collect();
    names.sort_by_key(|name| {
        ORDINARY
            .iter()
            .position(|known| known == name)
            .unwrap_or(ORDINARY.len())
    });
    names.dedup();
    let mut journeys = Vec::new();
    let mut issues = Vec::new();
    for name in names {
        let mut attempts: Vec<&serde_json::Value> = reports.iter().filter(|report| report["name"] == name).collect();
        attempts.sort_by_key(|report| report["attempt"].as_u64().unwrap_or(0));
        let count = |status: &str| attempts.iter().filter(|report| report["status"] == status).count();
        let (passed, failed, skipped) = (count("passed"), count("failed"), count("skipped"));
        let classification = if failed == 0 && passed > 0 {
            "pass"
        } else if failed > 0 && passed > 0 {
            "flaky"
        } else if failed > 0 {
            "fail"
        } else {
            "skipped"
        };
        let failures: Vec<_> = attempts
            .iter()
            .filter(|report| report["status"] == "failed")
            .map(|report| {
                serde_json::json!({
                    "attempt": report["attempt"],
                    "step": report["failing_step"],
                    "class": report["classification"],
                    "service": report["service"],
                    "detail": report["detail"],
                    "directory": report["directory"],
                })
            })
            .collect();
        let mut gaps: Vec<serde_json::Value> = Vec::new();
        for report in &attempts {
            for step in report["steps"].as_array().into_iter().flatten() {
                for gap in step["gaps"].as_array().into_iter().flatten() {
                    let entry = serde_json::json!({"step": step["id"], "check": gap["check"], "kind": gap["kind"],
                        "service": gap["service"], "note": gap["note"]});
                    if !gaps.contains(&entry) {
                        gaps.push(entry);
                    }
                }
            }
        }
        if classification != "pass" {
            // The latest non-passing attempt names the step to fix.
            if let Some(last) = attempts.iter().rev().find(|report| report["status"] != "passed") {
                issues.push(serde_json::json!({
                    "journey": name,
                    "result": classification,
                    "step": last["failing_step"],
                    "class": last["classification"].as_str().unwrap_or("skipped"),
                    "service": last["service"],
                    "detail": last["detail"],
                }));
            }
        }
        journeys.push(serde_json::json!({
                "name": name,
                "attempts": attempts.len(),
                "passed": passed,
                "failed": failed,
                "skipped": skipped,
                "classification": classification,
                "failures": failures,
                "gaps": gaps,
        }));
    }
    let mut not_wired: Vec<&str> = issues
        .iter()
        .filter(|issue| issue["class"] == "service_not_wired")
        .filter_map(|issue| issue["service"].as_str())
        .collect();
    not_wired.sort_unstable();
    not_wired.dedup();
    serde_json::json!({
        "schema_version": 1,
        "kind": "port-journey-summary",
        "platform": reports.first().map(|report| report["platform"].clone()),
        "journeys": journeys,
        "tracked_issues": issues,
        "services_not_wired": not_wired,
    })
}

fn summary_markdown(summary: &serde_json::Value) -> String {
    let os = summary["platform"]["os"].as_str().unwrap_or("unknown");
    let mut lines = vec![
        format!("### Port journeys on {os}"),
        String::new(),
        "| Journey | Passed | Failed | Skipped | Result | Step | Class | Service |".to_owned(),
        "| --- | ---: | ---: | ---: | --- | --- | --- | --- |".to_owned(),
    ];
    let issues = summary["tracked_issues"].as_array().cloned().unwrap_or_default();
    for row in summary["journeys"].as_array().into_iter().flatten() {
        let name = row["name"].as_str().unwrap_or("?");
        let issue = issues.iter().find(|issue| issue["journey"] == name);
        let field = |key: &str| {
            issue
                .and_then(|issue| issue[key].as_str())
                .unwrap_or_default()
                .to_owned()
        };
        lines.push(format!(
            "| {name} | {}/{} | {} | {} | {} | {} | {} | {} |",
            row["passed"],
            row["attempts"],
            row["failed"],
            row["skipped"],
            row["classification"].as_str().unwrap_or("?"),
            field("step"),
            field("class"),
            field("service"),
        ));
    }
    lines.push(String::new());
    if !issues.is_empty() {
        lines.push("Tracked issues:".to_owned());
        for issue in &issues {
            let detail: String = issue["detail"].as_str().unwrap_or("").chars().take(240).collect();
            lines.push(format!(
                "- {} {}: {} — {}",
                issue["journey"].as_str().unwrap_or("?"),
                issue["step"].as_str().unwrap_or("-"),
                issue["class"].as_str().unwrap_or("?"),
                detail.replace('|', "/")
            ));
        }
        lines.push(String::new());
    }
    let gaps: usize = summary["journeys"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| row["gaps"].as_array().map_or(0, Vec::len))
        .sum();
    lines.push(format!(
        "Checks not observable on this platform (listed per journey in summary.json): {gaps}."
    ));
    lines.push(String::new());
    lines.join("\n")
}

/// `cargo xtask journey summarize --results=<dir> [--results=<dir>...] [--output=<dir>] [--summary=<file>]`:
/// combine the `result.json` files below each results directory.
pub(super) fn summarize_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let roots: Vec<PathBuf> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix("--results="))
        .map(PathBuf::from)
        .collect();
    if roots.is_empty() {
        return Err("summarize needs at least one --results=<dir>".into());
    }
    let output = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--output="))
        .map_or_else(|| roots[0].clone(), PathBuf::from);
    let markdown = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--summary="))
        .map(Path::new);
    let mut reports = Vec::new();
    for root in &roots {
        collect_reports(root, 0, &mut reports)?;
    }
    if reports.is_empty() {
        return Err("no port journey result.json found".into());
    }
    write_summary(&output, &reports, markdown)?;
    Ok(())
}

/// Read every `result.json` of kind `port-journey-result` below `directory`
/// (a few levels: artifact folder, attempt folder).
fn collect_reports(
    directory: &Path,
    depth: usize,
    reports: &mut Vec<serde_json::Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    const DEPTH_LIMIT: usize = 4;
    const REPORT_LIMIT: usize = 1024;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() && depth < DEPTH_LIMIT {
            collect_reports(&path, depth + 1, reports)?;
        } else if metadata.is_file() && path.file_name().is_some_and(|name| name == "result.json") {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if value["kind"] == "port-journey-result" {
                if reports.len() == REPORT_LIMIT {
                    return Err(format!("more than {REPORT_LIMIT} journey results").into());
                }
                reports.push(value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, status: StepStatus) -> StepRecord {
        StepRecord {
            id: id.into(),
            status,
            observed: format!("{id} observed"),
            gaps: Vec::new(),
        }
    }

    fn platform() -> serde_json::Value {
        serde_json::json!({"os": "linux", "display": "xvfb"})
    }

    #[test]
    fn the_ordinary_list_is_the_manifest_ordinary_tier_in_order() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/e2e/journeys.json");
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
        let ordinary: Vec<&str> = manifest["journeys"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["tier"] == "ordinary")
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ordinary, ORDINARY);
    }

    #[test]
    fn the_first_failure_classifies_the_journey_and_names_its_service() {
        let mut first = step("s1", StepStatus::Pass);
        first.gaps.push(Gap {
            check: "exact editor text".into(),
            kind: GapKind::Unobservable(Service::Accessibility),
            note: "no accessibility tree".into(),
        });
        let steps = [
            first,
            step(
                "s2",
                StepStatus::Fail(Failure::not_wired(Service::Dialogs, "no folder chooser appeared")),
            ),
            step("s3", StepStatus::NotRun("Prerequisite step failed".into())),
        ];
        let report = report("workspace", 2, Path::new("/evidence/workspace-2"), platform(), &steps);
        assert_eq!(report["status"], "failed");
        assert_eq!(report["classification"], "service_not_wired");
        assert_eq!(report["service"], "dialogs");
        assert_eq!(report["failing_step"], "s2");
        assert_eq!(report["attempt"], 2);
        assert_eq!(
            report["steps"][1]["observed"],
            "service not wired (dialogs): no folder chooser appeared"
        );
        assert_eq!(report["steps"][0]["gaps"][0]["kind"], "unobservable");
        assert_eq!(report["steps"][0]["gaps"][0]["service"], "accessibility");
        assert_eq!(report["steps"][2]["status"], "NOT_RUN");
        assert!(outcome_line(&report).starts_with("FAIL workspace — s2 service_not_wired (dialogs): no folder"));
    }

    #[test]
    fn skipped_steps_and_gaps_do_not_fail_a_journey_but_all_skipped_is_skipped() {
        let mut gapped = step("s3", StepStatus::Pass);
        gapped.gaps.push(Gap {
            check: "menu checked state".into(),
            kind: GapKind::WindowsOnly,
            note: NATIVE_MENU_STATE.into(),
        });
        let passed = report(
            "split_clone_sync",
            1,
            Path::new("/e"),
            platform(),
            &[
                step("s1", StepStatus::Pass),
                step("s2", StepStatus::Skipped("portal absent".into())),
                gapped,
            ],
        );
        assert_eq!(passed["status"], "passed");
        assert_eq!(passed["classification"], serde_json::Value::Null);
        assert_eq!(passed["steps"][2]["gaps"][0]["kind"], "windows_only");
        assert_eq!(outcome_line(&passed), "PASS split_clone_sync");

        let skipped = report(
            "udl",
            1,
            Path::new("/e"),
            platform(),
            &[
                step("s1", StepStatus::Skipped("no XDG desktop portal".into())),
                step("s2", StepStatus::NotRun("Prerequisite step was skipped".into())),
            ],
        );
        assert_eq!(skipped["status"], "skipped");
        assert_eq!(skipped["detail"], "s1: no XDG desktop portal");
        assert_eq!(outcome_line(&skipped), "SKIP udl — s1: no XDG desktop portal");
    }

    #[test]
    fn every_failure_class_has_a_distinct_name_and_every_service_a_name() {
        let classes = [
            Class::ServiceNotWired(Service::FileWatching),
            Class::Environment,
            Class::Timeout,
            Class::Harness,
            Class::Product,
        ];
        let mut names: Vec<_> = classes.iter().map(|class| class.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), classes.len());
        let mut services: Vec<_> = Service::ALL.iter().map(|service| service.name()).collect();
        services.sort_unstable();
        services.dedup();
        assert_eq!(services.len(), Service::ALL.len());
        for reason in [NATIVE_MENU_STATE, WIN32_PROCESS_TREE] {
            assert!(reason.len() > 20, "a skip reason must say why: {reason}");
        }
        for service in Service::ALL {
            assert!(
                STAND_INS.iter().any(|(known, _, _)| *known == service),
                "{} has no stand-in probe",
                service.name()
            );
        }
    }

    #[test]
    fn the_seam_probe_reports_a_stand_in_until_its_phrase_is_gone() {
        let root = std::env::temp_dir().join(format!(
            "bareline-port-journey-seam-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let seam = root.join(SEAM);
        std::fs::create_dir_all(&seam).unwrap();
        assert_eq!(
            seam_stand_in(&root, Service::FileWatching),
            None,
            "an unreadable seam is unknown"
        );
        std::fs::write(seam.join("watch.rs"), "Err(unsupported_io(Capability::FileWatch))").unwrap();
        assert_eq!(seam_stand_in(&root, Service::FileWatching), Some(true));
        std::fs::write(
            seam.join("watch.rs"),
            "pub use bareline_platform_linux::LinuxWatchService as WatchService;",
        )
        .unwrap();
        assert_eq!(seam_stand_in(&root, Service::FileWatching), Some(false));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_summary_separates_pass_flaky_fail_and_skipped_and_lists_issues() {
        let pass = report(
            "plain_text",
            1,
            Path::new("/e"),
            platform(),
            &[step("s1", StepStatus::Pass)],
        );
        let watch = |attempt| {
            report(
                "huge_log_tail",
                attempt,
                Path::new("/e"),
                platform(),
                &[
                    step("s1", StepStatus::Pass),
                    step(
                        "s3",
                        StepStatus::Fail(Failure::not_wired(Service::FileWatching, "no append observed")),
                    ),
                ],
            )
        };
        let flaky = [
            report(
                "code_config",
                1,
                Path::new("/e"),
                platform(),
                &[step("s1", StepStatus::Fail(Failure::product("colours differ")))],
            ),
            report(
                "code_config",
                2,
                Path::new("/e"),
                platform(),
                &[step("s1", StepStatus::Pass)],
            ),
        ];
        let skipped = report(
            "udl",
            1,
            Path::new("/e"),
            platform(),
            &[step("s1", StepStatus::Skipped("no portal".into()))],
        );
        let reports = [watch(2), pass, flaky[1].clone(), watch(1), flaky[0].clone(), skipped];
        let summary = summarize(&reports);
        let journeys = summary["journeys"].as_array().unwrap();
        // Manifest order, not input order.
        let order: Vec<_> = journeys.iter().map(|row| row["name"].as_str().unwrap()).collect();
        assert_eq!(order, ["plain_text", "code_config", "huge_log_tail", "udl"]);
        let rows: serde_json::Map<String, serde_json::Value> = journeys
            .iter()
            .map(|row| (row["name"].as_str().unwrap().to_owned(), row.clone()))
            .collect();
        assert_eq!(rows["plain_text"]["classification"], "pass");
        assert_eq!(rows["code_config"]["classification"], "flaky");
        assert_eq!(rows["huge_log_tail"]["classification"], "fail");
        assert_eq!(rows["huge_log_tail"]["failures"][0]["attempt"], 1);
        assert_eq!(rows["udl"]["classification"], "skipped");
        assert_eq!(summary["services_not_wired"], serde_json::json!(["file watching"]));
        let issues = summary["tracked_issues"].as_array().unwrap();
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[1]["journey"], "huge_log_tail");
        assert_eq!(issues[1]["step"], "s3");
        assert_eq!(issues[2]["class"], "skipped");
        let markdown = summary_markdown(&summary);
        assert!(markdown.contains("| huge_log_tail | 0/2 | 2 | 0 | fail | s3 | service_not_wired | file watching |"));
        assert!(markdown.contains("| plain_text | 1/1 | 0 | 0 | pass |"));
    }

    #[test]
    fn summarize_reads_attempt_results_below_artifact_folders() {
        let root = std::env::temp_dir().join(format!(
            "bareline-port-journey-summary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let attempt = root.join("port-journey-linux-udl/udl-1");
        std::fs::create_dir_all(&attempt).unwrap();
        let value = report("udl", 1, &attempt, platform(), &[step("s1", StepStatus::Pass)]);
        std::fs::write(attempt.join("result.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        // Unrelated JSON named result.json (the Windows runner's) is ignored.
        let other = root.join("other/plain_text-1");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("result.json"), br#"{"status": "PASS"}"#).unwrap();
        let output = root.join("summary");
        summarize_command(&[
            format!("--results={}", root.display()),
            format!("--output={}", output.display()),
        ])
        .unwrap();
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert_eq!(summary["journeys"][0]["name"], "udl");
        assert_eq!(summary["journeys"][0]["passed"], 1);
        assert_eq!(summary["journeys"].as_array().unwrap().len(), 1);
        assert!(output.join("summary.md").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}
