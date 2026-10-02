# Roadmap

This roadmap lists what remains between the current Windows x64 preview and Bareline 1.0, and what is planned after it. It follows the project's implementation plan of 2026-09-30, after the Phase 0 to 4 fixes it describes were merged. Bareline has one maintainer, so no dates are promised; items move to the [CHANGELOG](CHANGELOG.md) when they ship. What is implemented and verified today is in [docs/STATUS.md](docs/STATUS.md).

## Next preview: qualification of the merged fixes

The fixes merged for the Phase 0 to 4 review findings are implemented and covered by automated tests, but most have not been checked on real machines. The next preview release is about that evidence.

- **Release-build benchmark.** Run the Bareline vs Notepad++ harness on the reference machine from a release build, under the FC-10 protocol (10 runs, warm and cold cache), and publish the table in [docs/perf](docs/perf/README.md). Until then, no comparative speed or memory claim is made.
- **Native journeys green on the tag.** Re-run the four journeys that passed before the merged changes, and bring the quarantined journeys (large log, workspace, UDL, macro and external command, split view, portable, UI regressions) to three consecutive nightly passes ([readiness ledger](docs/qa/READINESS.md)).
- **Recovery and install acceptance on disposable VMs.** Run the `crash_recovery` and `install_update_rollback` journeys, including killing the editor at every journal and checkpoint boundary.
- **Windows version floor.** Test the installer and editor on clean VMs of Windows 10 1809 (build 17763, which LTSC 2019 and Server 2019 use), 21H2 (19044, LTSC 2021) and 22H2 (19045), and on Windows 11. The installer currently refuses builds older than 19045; the floor changes only after those tests. Windows 10 reached end of support in October 2025, and the consumer Extended Security Updates program ends on 2026-10-13, so the supported-versions statement will be reviewed together with this work.
- **Screenshots and reproducible demos** for the README: a crash and restore through the Recovery Center, and opening, searching and following a multi-gigabyte log.

## Beta (0.9)

- **Code signing.** Submit the SignPath Foundation application ([CODE_SIGNING.md](CODE_SIGNING.md)), with Azure Trusted Signing as the fallback; sign the setup, the executables and the uninstaller. Then add Minisign signatures for `SHA-256SUMS` and a winget listing, and publish the Scoop manifest in a bucket.
- **Working updates.** Enable the update pipeline in configured releases once signed releases exist, with its rollback.
- **All 13 native journeys** passing three times in CI, and fuzzing nightly for four weeks without an open crash.
- **24-hour soak** of the release build ([soak harness](tests/soak/README.md)).
- **Known issues** in the [README](README.md#known-issues-in-this-preview): network save, the 64 MiB context for anchored regular expressions, highlighting after several quick edits, settings rows that have no effect yet, spell check in large-file mode and in the second split pane, and the remaining large-file fold mapping delay.

## 1.0

- **Physical accessibility pass:** Narrator, NVDA and a JAWS spot check through open, edit, find, conflict and recovery; high contrast; Japanese, Chinese and Korean input method editors; AltGr and dead keys; 100 to 250% scaling and mixed-DPI monitors.
- **72-hour soak** with handle, memory and thread tracking.
- **Independent acceptance** of the blueprint acceptance cases, or explicit, published waivers.
- **Localization packs.** The user interface is English only today; command and menu text already goes through a resource table ([localization readiness](docs/localization.md)). Finish externalizing the remaining strings and accept about five community language packs before any non-English marketing.
- **Documentation and community:** the user guide, CHANGELOG, status and parity pages kept current; trademark clearance; a second maintainer for reviews.

## After 1.0

- Folder comparison and an inline diff view.
- Notepad++ parity gaps listed in [docs/PARITY.md](docs/PARITY.md), such as line delete and transpose, incremental search, case-changing replacement escapes, and more built-in languages.
- An ARM64 release; ARM64 executables are already built in CI.
- A decision on third-party extensions. Bareline 1.0 ships without plugins; the sandboxed extension host in the repository stays disabled until its containment and signing are complete.
