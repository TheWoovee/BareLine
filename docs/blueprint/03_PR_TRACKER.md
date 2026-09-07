# Bareline PR Tracker

**Last initialized:** 2026-09-05

Status values: `NOT_STARTED`, `IN_PROGRESS`, `DONE`, `BLOCKED`, `N/A`.

| PR | Title | Selected | Implemented | Verified | Tested | Accepted | Evidence / Notes |
|---|---|---|---|---|---|---|---|
| PR-001 | Workspace Scaffold, Command Core and Windows Shell | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-002 | Paged Document Engine and Core Undo | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-003 | Virtualized Editor Surface, Input and Selection Model | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-004 | File Lifecycle, Tabs, Sessions and Crash Recovery | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-005 | Search, Replace and Results Engine | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-006 | Power Editing: Multi-Cursor, Column, Lines and Bookmarks | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-007 | Encoding, EOL and Unicode Fidelity | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-008 | Language Catalog, Syntax Highlighting, Folding and UDL | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-009 | Workspace Explorer, Document List, Outline and Document Map | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-010 | Split Views, Tab Management and Synchronized Scrolling | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-011 | Menus, Command Palette, Shortcuts, Toolbar and Context Actions | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-012 | Settings, Themes, Localization and DPI | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-013 | Completion, Smart Typing and Language-Aware Editing | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-014 | Macros, External Commands and Output Panel | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-015 | File Watching, Monitoring/Tail and Safe Remote Paths | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-016 | Extension Platform and Isolated Plugin Manager | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-017 | Built-In Utilities, Compare Workspace, Export and Print | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-018 | Windows Integration, CLI, Installer, Portable Mode, Tray and Updater | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-019 | Huge-File Optimization and Performance Benchmark Suite | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-020 | Security, Recovery and Update Hardening | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-021 | Windows Product Polish, Parity Matrix and Release QA | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-022 | Cross-Platform Readiness and Linux/macOS Adapter Skeletons | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-023 | UI Primitives and Controls | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-024 | Accessibility and UI Automation | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-025 | Diff Core Engine | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-026 | Replace in Files and Workspace Replace | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |
| PR-027 | First-Party Extensions: JSON, XML and Hex | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED | NOT_STARTED |  |

## Update rules

- The implementation agent changes **Selected** to `DONE` when the PR is assigned/started.
- After code is complete and locally reviewed, set **Implemented** to `DONE` and add commit/branch reference.
- A separate verification pass sets **Verified** to `DONE` only after acceptance criteria are inspected, not merely because tests are green.
- **Tested** becomes `DONE` when the PR-specific required test set passes and evidence is recorded.
- **Accepted** is the final human/controller approval state. Do not self-accept unless the workflow explicitly assigns that responsibility.
- Never mark later columns `DONE` to compensate for an earlier blocked state. Use notes.
- Update only the row(s) actually worked on.
- Performance numbers never gate a status change; record measurements as evidence only (ADR-10).

## Evidence format

Use concise entries such as:

```text
impl: commit abc123; verify: reviewer run V-2026-09-10; tests: cargo test -p bareline-document (412 passed); perf: perf/open_1gb_20260910.json
```

## Blueprint revision v1.2

All 27 briefs now reference atomic acceptance cases and foundation contracts. Documentation updates do not alter implementation status. Record unresolved integration separately from completed slices; a stub alone does not complete a feature package. [Acceptance register](10_ACCEPTANCE_AND_TRACEABILITY.md) contains the pending evidence cases.
