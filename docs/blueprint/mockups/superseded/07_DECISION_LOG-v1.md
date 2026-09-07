# Bareline Decision Log (Architecture Decision Records)

**Version:** 1.2  
**Date:** 2026-09-05  
**Authority:** these decisions override any conflicting sentence elsewhere in this package. If a PR file and this log disagree, this log wins and the PR file is fixed.

## Owner constraints that drove every decision

1. **Faster and lighter than Notepad++, and loads better.** Measured on the same machine, same files. Performance is designed in from PR-001, not tuned in at PR-019.
2. **No developer-blocking gates.** Performance numbers are targets that are measured continuously and published. They never block a PR merge. Release readiness is a published comparison table, not a pass/fail wall.
3. **Open source, owned and distributed by the product owner.** Licensing, signing, governance and distribution are specified in `06_OPEN_SOURCE_AND_DISTRIBUTION.md`.

## ADR index

| ID | Decision | Affects |
|---|---|---|
| ADR-01 | Resident-or-paged byte source, no mmap in v1 | PR-002, PR-015, PR-019 |
| ADR-02 | Valid UTF-8 text view and lossless original bytes | See foundation contracts and assigned PRs |
| ADR-03 | Source generation and unavailable data | See foundation contracts and assigned PRs |
| ADR-04 | Native Win32 menu bar, context menus and common dialogs; custom-rendered everything else | PR-001, PR-011, PR-023 |
| ADR-05 | Dedicated UI-primitives PR (PR-023) | PR-001, PR-023 |
| ADR-06 | Accessibility via AccessKit in its own PR (PR-024) | PR-012, PR-024 |
| ADR-07 | `RecordingBackend` headless renderer ships in PR-001 | PR-001, PR-022 |
| ADR-08 | `panic = "abort"` in release with metadata-only crash handler | PR-001, PR-004 |
| ADR-09 | Approved and forbidden dependency list | all |
| ADR-10 | Performance policy: targets, budgets, continuous measurement, no merge gates | 01 §7, PR-001, PR-019 |
| ADR-11 | DocumentService is a logical actor | See foundation contracts and assigned PRs |
| ADR-12 | Diff engine lives in `crates/diff`, delivered by PR-025 in wave 2 | PR-004, PR-017, PR-025 |
| ADR-13 | Comment toggle is owned by PR-013; PR-006 registers IDs and hook only | PR-006, PR-013 |
| ADR-14 | Tabs: PR-004 owns model and persistence, PR-010 owns UI | PR-004, PR-010 |
| ADR-15 | `crates/commands` is owned by PR-011; other PRs register from their own crates | PR-006, PR-011 |
| ADR-16 | Replace-in-files moves to PR-026 | PR-005, PR-026 |
| ADR-17 | Lexilla primary, native UDL engine is the fallback lexer | PR-008 |
| ADR-18 | Bounded regex windows and completeness | See foundation contracts and assigned PRs |
| ADR-19 | PR-012 keeps settings, themes, localization and DPI; accessibility moves out | PR-012 |
| ADR-20 | Optional extension runtime lifecycle | See foundation contracts and assigned PRs |
| ADR-21 | Persisted formats: TOML for human-edited, JSON for machine-written state, binary for journals | PR-004, PR-012 |
| ADR-22 | Parity additions for v1 | PR-004, PR-005, PR-006, PR-008, PR-009 |
| ADR-23 | Clipboard history: opt-in, in-memory only, off by default | PR-006 |
| ADR-24 | Local history deferred to v1.1 | 00, 01 |
| ADR-25 | First-party extension seed set in PR-027 | PR-027 |
| ADR-26 | Compare: inline view and folder compare deferred to v1.1 | PR-017 |
| ADR-27 | Integrated terminal and source control stay excluded | mocks |
| ADR-28 | English at v1; community language packs | PR-012 |
| ADR-29 | Brand stays monoline `b`, cyan-teal accent; exact tokens defined | 00_BRAND, 08 |
| ADR-30 | Superseded boards and current concepts | See foundation contracts and assigned PRs |
| ADR-31 | Installer: Inno Setup per-user default, portable ZIP, winget manifest; MSIX later | PR-018 |
| ADR-32 | Windows renderer: Direct2D hardware default with mandatory software render-target mode; default chosen by measurement | PR-001, PR-019 |
| ADR-33 | Startup budget: nothing but settings, keymap and session manifest are read before first frame | PR-001, PR-004 |
| ADR-34 | License: MPL-2.0 core, MIT OR Apache-2.0 for extension SDK/protocol, trademark policy for name and logo | 06 |
| ADR-35 | Signing: Windows Authenticode via SignPath Foundation or Azure Trusted Signing; update manifests via minisign (ed25519) | PR-018, PR-020 |
| ADR-36 | Governance: owner-led, DCO sign-off, GitHub Actions CI, monthly releases | 06 |
| ADR-37 | Text/raw domains and codec provenance | FC-01, all affected PRs |
| ADR-38 | Bounded sources, newline aggregates and storage budgets | FC-02, all affected PRs |
| ADR-39 | Durable recovery and baseline availability | FC-03, all affected PRs |
| ADR-40 | Save, tail continuity and filesystem semantics | FC-04, all affected PRs |
| ADR-41 | Complete search and revalidated workspace replacement | FC-05, all affected PRs |
| ADR-42 | Logical actors, linked undo and bounded diff | FC-06, all affected PRs |
| ADR-43 | Extension capabilities, lifecycle and offline contracts | FC-07, all affected PRs |
| ADR-44 | Release trust, rollback protection and signing ceremony | FC-08, all affected PRs |
| ADR-45 | Platform floor, path fidelity and workspace trust | FC-09, all affected PRs |
| ADR-46 | Reproducible benchmark method and claim boundaries | FC-10, all affected PRs |

---

## ADR-01 Resident-or-paged byte source, no mmap in v1

**Problem.** A memory-mapped base file crashes the process with an in-page exception if another process truncates it, blocks log rotation and other editors' atomic replace, and behaves badly on network shares.

**Decision.** `ByteSource` has two implementations behind one trait:

- `Resident`: the file is streamed into owned immutable chunks. Viewport pages are read first and published immediately; the rest fills in the background. Default for files up to **256 MiB** (setting `document.resident_max_bytes`). Typical files therefore behave like Notepad++ with respect to external truncation while opening faster because the first viewport does not wait for the whole read.
- `Paged`: above the threshold, pages are read on demand into a bounded LRU cache. File is opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`.

Memory mapping is not used in v1. It may be added later as a third `ByteSource` implementation behind the same trait, never as the public abstraction.

**Consequences.** PR-002 "opening must not copy the file" is rephrased: opening must not copy the file **before the first viewport is interactive**, and files above the threshold are never fully materialized in RAM. Huge-file targets are unchanged.

## ADR-02 Valid UTF-8 text view and lossless original bytes

**Decision, revised v1.2.** Use a valid UTF-8 text view with tagged opaque spans retaining original bytes and a separate original-byte domain. Never put invalid UTF-8 in a valid-text API or reserve user code points as escape sentinels. Unchanged spans copy original bytes on same-encoding save; changed spans use a supported encoder. Large transcodes use disk-backed storage. See FC-01/02.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-03 Source generation and unavailable data

**Decision, revised v1.2.** Paged and still-loading Resident sources can become unavailable if their original generation changes. Owned bytes remain usable; missing regions never silently read a new generation. Only sealed Resident data remains completely available. Save/Save As require a complete source. Export available data is a separate action with a gap report. Tail append uses FC-04 continuity, not the ordinary size-change rule.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-04 Native chrome where it is free, custom where it matters

**Decision.** On Windows the menu bar, context menus, message boxes and file open/save/folder dialogs are native Win32 through `PlatformServices`. The tab strip, editor, panels, status bar, find bar, command palette, settings pages, tooltips and popovers are custom-rendered Bareline views. Menus are still built from the command registry; the native HMENU is a projection. Linux and macOS adapters supply native equivalents later.

## ADR-05 UI primitives get their own PR

**Decision.** New **PR-023 UI Primitives and Controls** in wave 1, depending on PR-001. It owns `crates/ui` controls: button, toggle, checkbox, radio, combo, text field with IME and clipboard, slider, virtual list, virtual tree, scrollbar, splitter, tab strip, tooltip, popover, banner. PR-001 ships only the retained-tree foundation.

## ADR-06 Accessibility through AccessKit in PR-024

**Decision.** New **PR-024 Accessibility and UI Automation** in wave 5, depending on PR-023, PR-003 and PR-011. Uses `accesskit`, `accesskit_winit` and `accesskit_windows`. Editor exposes caret, selection and visible text through bounded text-pattern APIs. PR-012 no longer owns accessibility.

## ADR-07 Headless renderer from day one

**Decision.** PR-001 ships `RecordingBackend`: a `RenderBackend` that records draw commands and uses deterministic monospace metrics for text shaping. UI model, layout, hit-testing and keybinding tests run against it on every OS in CI. PR-022 may add a real software painter later for Linux and macOS previews.

## ADR-08 Abort on panic in release

**Decision.** Release profile uses `panic = "abort"`. Recovery depends on journal health and baseline availability (FC-03). Report the last durable edit and baseline readiness; never promise a maximum loss interval when storage is unavailable. A crash handler writes version, build hash, panic location and backend state to the diagnostics log, never document content. Debug builds unwind.

## ADR-09 Approved and forbidden dependencies

**Approved in the editor process (`bareline.exe`).**

| Purpose | Crate | Note |
|---|---|---|
| Windows APIs | `windows` (windows-rs), pinned | only inside `platform-windows` and `renderer-d2d` |
| Window and input | `winit` 0.30.x pinned | |
| Threads and channels | `std::thread`, `std::sync::mpsc` or `crossbeam-channel` | no async runtime in the editor process |
| Parallel pools | `rayon` | search, workspace scan, hashing |
| Errors | `thiserror` in libraries, `anyhow` only in binaries | |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` | |
| Serialization | `serde`, `toml_edit` (settings, comment-preserving), `serde_json` (state), `postcard` (RPC) | |
| Regex | `pcre2` (bundled source) | |
| Byte search | `memchr`, `aho-corasick` | |
| Unicode | `unicode-segmentation`, `unicode-width`, `unicode-normalization` (optional command only) | |
| Encodings | `encoding_rs` | |
| Hashes and checksums | `crc32c`, `sha2`, `sha1`, `md-5`, `base64`, `percent-encoding` | |
| CLI parsing | `lexopt` | |
| Icons | `png` for pre-rasterized sprite sheets built by `xtask` from SVG sources | no SVG renderer at runtime |
| Accessibility | `accesskit`, `accesskit_winit`, `accesskit_windows` | |
| Lexilla bridge | `cxx`, `cc` or `cmake` | |
| Signatures | `minisign-verify` (or `ed25519-dalek`) for manifests; Authenticode through `WinVerifyTrust` | |
| HTTP for update check | WinHTTP through `windows` | no `reqwest`, no bundled TLS |
| Archives | `zip` behind a safe-extraction wrapper | |
| File watching | own `ReadDirectoryChangesW` wrapper | not `notify` |

**Approved only in `bareline-extension-host.exe`:** `wasmtime` with component model, `wasmtime-wasi`.

**Forbidden in the editor process:** `tokio`, `async-std`, `smol`, any web view or browser engine, `egui`, `iced`, `gpui`, `druid`, `reqwest`, `openssl`, `resvg`, `image` (beyond `png`), any crate that spawns background threads at load time.

Adding a dependency requires a one-line justification in the PR description and a `cargo deny` pass for licenses and advisories.

## ADR-10 Performance policy

**Decision.** Section 7 of the requirements is rewritten as follows.

- **Product targets** are comparisons against current stable Notepad++ with plugins disabled, on the same machine and same fixtures. Time metrics target **≤ 0.8×** Notepad++ P50. Idle private bytes target **≤ 1.0×** Notepad++ P50 with an engineering budget of 25 MB. Working set is reported but not targeted because shared DLL pages dominate it.
- **Engineering budgets** are absolute numbers agents design against (cold launch 250 ms, warm 120 ms, 10 MB open 100 ms, 1 GB open 1.0 s, edit-to-paint 8 ms P95, idle 25 MB private bytes, installer 15 MB).
- **Measurement is continuous.** PR-001 ships the `xtask perf` skeleton with launch time and idle private bytes. A nightly CI job runs it and appends results to `tests/perf/results/`. A regression above 10 percent opens a tracking issue automatically. PR-019 completes the scenario set and does the profiling pass.
- **No merge gate.** A PR is never blocked on a performance number. It is blocked only on correctness, tests, lint and the architecture rules that make performance possible (no full-file materialization, no UI-thread I/O, no forbidden dependencies).
- **Release readiness** is a published table with raw data and methodology. Marketing may say "faster and lighter" only for metrics where the table shows it.

## ADR-11 DocumentService is a logical actor

**Decision, revised v1.2.** PR-002 owns one logical actor per document scheduled on a bounded shared pool. An actor does not own an OS thread. Queues, snapshot pins, result batches and undo memory have explicit budgets. See FC-06.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-12 Diff engine in `crates/diff`, PR-025 in wave 2

**Decision.** New **PR-025 Diff Core Engine** depends on PR-002 only and runs in wave 2 beside PR-003. It delivers `CompareOptions`, `DiffHunk`, `CompareResult`, bounded patience/histogram plus Myers refinement, intraline refinement, coarse fallback and cancellation, all OS-neutral. PR-004 uses it for conflict and recovery previews. PR-017 builds the Compare Workspace UI on it.

## ADR-13 Comment toggle owned by PR-013

**Decision.** PR-006 registers `editor.comment.toggleLine`, `editor.comment.toggleBlock` and a `CommentProvider` hook that returns "unavailable" until PR-013 supplies language metadata. PR-013 implements the behavior.

## ADR-14 Tabs split

**Decision.** PR-004 owns `TabModel`, pin state, order, close policies as model operations, recent files and persistence. PR-010 owns the tab strip UI: drag reorder, keyboard reorder, overflow, colors, vertical tabs, MRU switcher UI and drag between views.

## ADR-15 Commands crate ownership

**Decision.** PR-011 owns `crates/commands`. Other PRs add commands by calling a `register_commands(&mut CommandRegistry)` function from their own crate. PR-006 therefore edits `crates/editor-surface`, not `crates/commands`.

## ADR-16 Replace-in-files moves to PR-026

**Decision.** PR-005 delivers search in current document, open documents and folders, plus replace in the current document, selection and open documents. New **PR-026 Replace in Files and Workspace Replace** depends on PR-005, PR-004 and PR-007, runs in wave 5, and owns preview, per-file atomic write, encoding and EOL preservation, and the summary report.

## ADR-17 Lexilla primary, native UDL engine as fallback

**Decision.** Lexilla is the primary lexer for breadth. The Bareline UDL engine is implemented natively in Rust and doubles as the fallback lexer. PR-008 must ship catalog entries for the fifteen explicitly listed v1 languages in FC-09 in both forms. If viewport-first Lexilla styling for a language cannot show a styled first viewport within 100 ms on the 1 GB fixture, the native definition is used for that language by default. Per-line lexer state is stored sparsely at checkpoints every N lines, and styling restarts from the nearest checkpoint or a blank-line anchor.

## ADR-18 Bounded regex windows and completeness

**Decision, revised v1.2.** Scan bounded UTF-8-aligned windows, preferably ending at line breaks but able to split arbitrarily long lines. The PCRE2 adapter must preserve anchors, lookbehind and partial-match context. Pattern-string syntax heuristics do not prove correctness. UnsupportedStreaming and resource limits are visible incomplete results and cannot drive replacement. See FC-05.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-19 PR-012 scope

**Decision.** PR-012 keeps settings, themes, localization resource system and DPI. Accessibility moves to PR-024.

## ADR-20 Optional extension runtime lifecycle

**Decision, revised v1.2.** Wasmtime runs in a separate signed host, obtained through verified download or offline package. Disabling extensions stops the host; explicit Remove runtime removes its files. PR-001 defines the verified-package contract; PR-016 implements offline installation and a fail-closed online stub; PR-018 supplies the verified online adapter. See FC-07/08.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-21 Persisted formats

**Decision.** TOML for anything a human edits: settings, themes, language catalog, UDL definitions, macros, keymaps. JSON for machine-written state: session, recovery manifest, catalog index, perf results. Binary with CRC32C for recovery journals. All versioned.

## ADR-22 Parity additions for v1

| Feature | Owner |
|---|---|
| Column Editor (insert number sequence with padding/step, or repeated text) | PR-006 |
| Multi-style Mark: five independent mark styles, clear one or all | PR-005 |
| Hide Lines and Show hidden lines | PR-006 |
| Restore Last Closed Tab, Reload From Disk, Set Read-Only toggle | PR-004 |
| Paste Special (plain text, no formatting) and drag-and-drop text move | PR-006 |
| Fold All, Unfold All, Fold Level 1..8 | PR-008 |
| Import Notepad++ `functionList` parser definitions where mappable | PR-009 |

Character Panel, Post-It mode and Document Peeker are v1.1.

## ADR-23 Clipboard history

**Decision.** Optional clipboard history, off by default, in-memory only, cleared on exit, capped at 20 entries, never persisted. FR-029 wording is updated.

## ADR-24 Local history deferred

**Decision.** Per-file local history is v1.1. The research doc marks it as a future distinction.

## ADR-25 First-party extension seed set

**Decision.** New **PR-027 First-Party Extensions** in wave 7, depending on PR-016: JSON tools (format, minify, validate, tree view), XML tools (format, validate, XPath), Hex view (read-only viewer with bytes and text panes). They double as the SDK reference implementations and live in `extensions/` under MIT OR Apache-2.0.

## ADR-26 Compare scope

**Decision.** Inline (unified) view and folder compare are v1.1. Marketing copy must not promise them for v1.

## ADR-27 Terminal and source control stay excluded

**Decision.** Confirmed excluded from core. Mocks showing them are marketing boards only and must not be used as UI references.

## ADR-28 Localization at launch

**Decision.** English ships in v1 with the full resource system. Community packs are accepted through the open-source repo.

## ADR-29 Brand tokens

**Decision.** Keep the monoline lowercase `b` mark and cyan-teal accent. Exact tokens are now in `00_BRAND_AND_LOGO.md` and reused by themes and by every Imagen prompt.

## ADR-30 Superseded boards and current concepts

**Decision, revised v1.2.** All_Screens.png and Comparison.png are obsolete exploration boards, unsuitable for UI or performance marketing claims. Current references are indexed in mockups/README.md. Written interaction contracts control behavior; generated images remain visual references with provenance and limitations.

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) provide the normative details.

## ADR-31 Installer

**Decision.** Inno Setup script producing a per-user installer by default with an optional per-machine mode, a portable ZIP, a winget manifest and SHA-256SUMS plus minisign signature for every artifact. MSIX for the Microsoft Store is v1.1.

## ADR-32 Renderer mode

**Decision.** Direct2D hardware rendering is the default. PR-001 must also implement the Direct2D software render-target mode. `xtask perf` measures cold launch and idle private bytes in both. If software mode is within 10 percent on frame time for the standard scroll workload and saves more than 5 MB idle, PR-019 flips the default to software. Either way the mode is a setting.

## ADR-33 Startup budget

**Decision.** Before the first frame the process may: parse the CLI, read `settings.toml`, read the keymap, read the session manifest header, create the window and renderer. It may not: restore tabs, read documents, enumerate fonts beyond resolving the editor font family, rasterize icons for hidden toolbars, touch the network, start the extension host, or read recent-file metadata. Session tabs restore incrementally after the first frame, active tab first.

## ADR-34 License

**Decision.** Application and core crates: **MPL-2.0** (file-level copyleft: changes to Bareline files must be shared; combining with other code, including proprietary extensions, is allowed). Extension SDK, protocol crates and first-party extensions: **MIT OR Apache-2.0**. The name "Bareline", the logo and the icon are trademarks of the owner and are not covered by the code license; forks must rename. Third-party notices are generated by `cargo about`. Switching the core to Apache-2.0 OR MIT later is a one-line change if the owner prefers maximum permissiveness.

## ADR-35 Signing

**Decision.** Windows Authenticode through SignPath Foundation (free for open-source projects) with Azure Trusted Signing as fallback. Update manifests and release checksums are signed with a minisign ed25519 key held offline by the owner; the public key is embedded in the binary; rotation procedure documented in `SECURITY.md`.

## ADR-36 Governance and process

**Decision.** Owner-led project with a maintainers file. Contributions require DCO sign-off, not a CLA. GitHub Actions CI. Monthly minor releases, patch releases as needed. Private vulnerability reporting through GitHub. No telemetry. Details in `06_OPEN_SOURCE_AND_DISTRIBUTION.md`.

## ADR-37 Text/raw domains and codec provenance

**Decision.** Adopt FC-01 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-38 Bounded sources, newline aggregates and storage budgets

**Decision.** Adopt FC-02 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-39 Durable recovery and baseline availability

**Decision.** Adopt FC-03 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-40 Save, tail continuity and filesystem semantics

**Decision.** Adopt FC-04 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-41 Complete search and revalidated workspace replacement

**Decision.** Adopt FC-05 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-42 Logical actors, linked undo and bounded diff

**Decision.** Adopt FC-06 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-43 Extension capabilities, lifecycle and offline contracts

**Decision.** Adopt FC-07 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-44 Release trust, rollback protection and signing ceremony

**Decision.** Adopt FC-08 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-45 Platform floor, path fidelity and workspace trust

**Decision.** Adopt FC-09 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).

## ADR-46 Reproducible benchmark method and claim boundaries

**Decision.** Adopt FC-10 in [09_FOUNDATION_CONTRACTS.md](09_FOUNDATION_CONTRACTS.md) as the normative v1.2 contract. This is specified behavior, pending implementation and [acceptance evidence](10_ACCEPTANCE_AND_TRACEABILITY.md).
