# Bareline Implementation Document

**Version:** 1.2  
**Date:** 2026-09-05  
**Purpose:** implementation architecture and PR execution reference  
**Decisions:** `07_DECISION_LOG.md` records every architecture decision (ADR-01 to ADR-36) and overrides this document where they differ  
**Primary language:** Rust 2024 edition  
**License:** MPL-2.0 for the application and core crates; MIT OR Apache-2.0 for extension SDK, protocol crates and first-party extensions  
**Window/input layer:** `winit 0.30.x` (pin the latest stable 0.30 patch used by the repo)  
**Windows renderer:** Direct2D 1.1 + DirectWrite through `windows-rs`, hardware and software modes, default chosen by measurement (ADR-32)  
**UI approach:** native Win32 menu bar, context menus and common dialogs through `PlatformServices`; a small Bareline retained UI layer for everything else; no browser runtime, no async runtime, no general-purpose widget toolkit

## 1. Architecture summary

Bareline is a Cargo workspace whose editor process stays small and deterministic. The UI thread renders only visible state. File I/O, search, syntax, workspace scanning and extensions are service tasks on plain threads that communicate through typed messages and immutable snapshots. Performance is a design input on every PR: the startup budget (§3.14), the dependency policy (§3.13) and continuous `xtask perf` measurement from PR-001 onward (ADR-10). No performance number blocks a merge.

```text
+------------------------- bareline.exe --------------------------+
| Bareline UI / commands / tabs / panels / EditorSurface        |
|       |                 |                    |                  |
| CommandRegistry    AppSessionModel      PlatformServices        |
|       |                 |                    |                  |
|       +--------+--------+--------------------+                  |
|                |                                               |
|        DocumentService <----> SearchService                    |
|          |    |   |             |                              |
|          |    |   +----> SyntaxService (Lexilla bridge)        |
|          |    +--------> Recovery/SessionService                |
|          +-------------> FileWatchService                       |
+----------------------------------------------------------------+
                 | named pipe / stdio RPC
+----------------v-------------------------+
| bareline-extension-host.exe              |
| WASM component runtime + capabilities    |
+------------------------------------------+
```

## 2. Repository layout

```text
/
  Cargo.toml
  Cargo.lock                 # committed; builds use --locked
  rust-toolchain.toml
  deny.toml                  # cargo deny: licenses, advisories, bans
  LICENSE                    # MPL-2.0
  LICENSE-SDK                # MIT OR Apache-2.0 for extension-sdk/, extensions-protocol/, extensions/
  TRADEMARKS.md
  CONTRIBUTING.md            # DCO sign-off, PR flow, dependency policy
  SECURITY.md                # private reporting, signing keys, rotation
  CODE_OF_CONDUCT.md
  .github/                   # CI workflows, issue/PR templates, nightly perf job
  crates/
    app/                 # composition root, command routing
    ui/                  # retained UI tree, layout, controls (PR-023), theme tokens
    a11y/                # AccessKit semantic tree adapter (PR-024)
    renderer/            # platform-neutral Painter/RenderBackend contracts
    renderer-recording/  # headless RecordingBackend for tests (PR-001)
    renderer-d2d/        # Direct2D/DirectWrite backend, hardware + software modes
    editor-surface/      # viewport/caret/selection/layout/input
    document/            # ByteSource (resident/paged), piece tree, line index, undo, DocumentService
    file-io/             # detection, transcoding, save/replace, open, metadata, watcher consumer
    search/              # literal, PCRE2, workspace search, results, mark styles
    diff/                # OS-neutral compare engine (PR-025)
    syntax/              # language catalog, Lexilla adapter, native UDL engine, folding
    workspace/           # folder tree, outlines, document list
    session/             # sessions, recovery journals, migrations
    commands/            # command registry, keybindings, macros (owned by PR-011)
    settings/            # TOML schemas/themes/preferences
    extensions-protocol/ # manifests, RPC messages, capability model (MIT OR Apache-2.0)
    extension-sdk/       # extension author SDK: WIT world, bindings, test harness (MIT OR Apache-2.0)
    platform/            # PlatformServices traits only
    platform-windows/    # Win32/windows-rs implementation, ReadDirectoryChangesW wrapper
    platform-linux/      # skeleton (PR-022)
    platform-macos/      # skeleton (PR-022)
    diagnostics/         # logs, perf counters, crash metadata
  native/
    lexilla-bridge/      # C++ shim (IDocument implementation) built through cxx
    pcre2/               # bundled source
  apps/
    bareline/            # Windows GUI binary
    extension-host/      # optional isolated extension host binary (separate download)
    update-helper/       # tiny apply/rollback helper
  extensions/            # first-party: json-tools/, xml-tools/, hex-view/ (PR-027)
  xtask/                 # perf harness, icon rasterization, release packaging
  ui-assets/
    icons/*.svg          # sources; rasterized to PNG sprite sheets by xtask
    themes/*.toml        # light.toml, dark.toml, high-contrast.toml, compare-cb.toml
  languages/
    catalog/*.toml
    udl-schema.json
  locales/               # en-US.toml at v1; community packs
  tests/
    fixtures/
    property/
    editing/
    encoding/
    diff/
    recovery/
    security/
    huge-files/
    e2e/
    perf/                # harness + results/*.json with machine metadata
  packaging/windows/     # Inno Setup script, portable zip, winget manifest
  docs/
    parity/
    extensions/          # SDK walkthrough
    adr/                 # copied from 07_DECISION_LOG.md
  release/               # release notes templates, checklists
```

## 3. Core technical choices

### 3.1 Rust core

Use Rust for memory safety, predictable ownership, cross-platform portability and direct OS access without a managed/browser runtime. Unsafe code is permitted only in narrowly audited FFI/platform modules and must be documented with invariants.

### 3.2 Small retained UI shell + native Windows renderer

Use `winit` only for cross-platform window creation, event delivery, DPI, keyboard/mouse/IME events and raw-window handles.

**Native where it is free (ADR-04).** The menu bar, context menus, message boxes and file/folder dialogs are native Win32 objects created through `PlatformServices`. They cost nothing to build, are accessible by default and look right on every Windows version. The menu is still a projection of the command registry.

**Custom where it matters.** Bareline owns a deliberately small retained UI layer for the controls this product actually needs: tab strip, splitters, virtual lists/trees, scrollbars, text fields, buttons/toggles, popovers, command palette, panels, find bar, status bar and settings pages. These controls are built once in PR-023 and reused everywhere (ADR-05). Do not build a general-purpose UI framework.

**Three renderer backends (ADR-07, ADR-32).** `renderer-d2d` implements the contract twice: a hardware Direct2D device context and a Direct2D software render target. Both ship; `renderer.mode` selects; PR-019 picks the default from measured idle memory and frame time. `renderer-recording` is a headless backend with deterministic monospace metrics used by every UI test on every OS.

On Windows, `renderer-d2d` creates the Direct2D device/context and DirectWrite factory lazily for the `HWND` supplied by winit, after the first frame is scheduled and never before settings are read. The renderer implements a neutral `RenderBackend`/`Painter` contract such as rectangles, lines, clips, images/icons, glyph/text layouts and layers. DirectWrite owns shaping, font fallback and glyph metrics on Windows; Direct2D owns drawing and device-loss recovery. Only the editor font family is resolved at startup; the system font collection is never enumerated eagerly. Icons are pre-rasterized PNG sprite sheets produced by `xtask` from SVG sources; no SVG renderer ships in the binary.

`EditorSurface` is a first-class Bareline view, not a toolkit text widget. It asks a `DocumentSnapshot` only for visible logical lines plus an overscan margin, shapes only the visible runs, caches layouts by `(document_revision, line, wrap_width, font_key, syntax_revision)`, and paints carets/selections/whitespace/decorations itself. The document model therefore remains completely independent of rendering.

Input flow is: winit event → normalized `UiEvent` → focused view → command/edit transaction. Winit IME preedit/commit events feed an `ImeSession`; the Windows adapter positions the candidate window/caret using native coordinates as needed. Accessibility is exposed through a platform-neutral semantic tree with AccessKit or direct platform adapters; do not derive accessibility from pixels.

The Windows backend is allowed to use `windows-rs` and COM types internally. No Direct2D/DirectWrite/Win32 type may appear in `document`, `search`, `syntax`, `session`, `commands`, `extensions-protocol`, or public neutral UI contracts.

Why this choice: it adds some UI code, but avoids an embedded browser, avoids a heavyweight renderer dependency, uses Windows' optimized text stack, permits true editor virtualization, and keeps the later Linux/macOS work constrained to renderer/platform adapters rather than the document/search engine.

### 3.3 Document storage: resident-or-paged byte source + piece tree (ADR-01, ADR-02)

Document owns a persistent piece root, generation-tagged sources, bounded edit/undo stores, sparse line index, Revision, ContentStateId, encoding metadata and raw/text provenance. Small files fill Resident chunks viewport-first; larger files use Paged sources. Non-UTF-8 decoding uses a disk-backed TranscodedSource and bounded mapped pages. No mmap in v1.

Sources return Ready, Pending or Unavailable without synchronous UI I/O. Only fully copied and validated Resident sources are sealed. Inverse bytes become owned before committing; cached pages alone are not undo storage. Snapshot immutability applies to owned data and root identity, not missing external bytes.

Piece aggregates carry Known/Unknown measures and first/last separator state. CRLF counts once even across pieces; LF and CR also count. An empty document has one line; a trailing separator adds the final empty line. See [FC-01/02/03](09_FOUNDATION_CONTRACTS.md).

### 3.4 Snapshot model

DocumentSnapshot carries revision, ContentStateId, source generation, root, owned stores and availability. It pins owned immutable spans within the global budget. Background results carry revision and generation. Consumers handle Unavailable explicitly, discard stale results, or obtain sealed input for operations requiring complete immutable data.

### 3.5 Search

Search consumes the valid text view. Case folding retains a mapping back to TextOffset; original byte offsets are separate. Windows are at most 16 MiB and split at UTF-8 boundaries, preferring line breaks. Long lines never force unbounded allocation.

The PCRE2 adapter preserves anchors, lookbehind and partial-match context according to a tested supported-pattern catalog. Unsupported semantics and exhausted 64 MiB context budgets produce explicit incomplete results. Cancellation, failures, stale sources and resource limits are distinct. Destructive replace requires complete revision-valid results; large edit batches stage on disk. PR-005 owns document/open-document replacement; PR-026 owns workspace preview, identity/hash revalidation and receipts. See [FC-05](09_FOUNDATION_CONTRACTS.md).

### 3.6 Syntax and folding

Use Lexilla as a lexer library, **not Scintilla as the editor widget**. Implement an `IDocument` adapter (about two dozen virtual methods, fuzzed at the FFI boundary) that exposes UTF-8 text ranges to Lexilla. Syntax service requests ranges surrounding viewport and edit regions. Lexer state is stored sparsely at checkpoints every N lines (default 256); styling restarts from the nearest checkpoint or a blank-line anchor, so a 1 GB file shows themed text immediately and correct styles progressively. Store token spans by revision/range as compact run arrays.

**Native fallback (ADR-17).** The Bareline UDL engine is implemented in Rust and doubles as a lexer. The fifteen most common languages ship both a Lexilla binding and a native definition. If Lexilla cannot style the first viewport of the 1 GB fixture within 100 ms for a language, that language defaults to the native definition. This keeps the huge-file promise independent of Lexilla's forward-only design.

Function-list extraction uses language-specific regex/parsers in separate definitions, imports Notepad++ `functionList` definitions where mappable, and is incremental.

### 3.7 Encoding (ADR-02)

Use the FC-01 codec matrix: encoding_rs for supported legacy codecs, dedicated streaming UTF-16/UTF-32 encoders and decoders, and true ISO-8859-1 handling. Detection is sampled and provisional; invalid spans use provenance, never reserved Unicode characters.

Unchanged same-encoding spans copy original bytes. Changed spans encode under an explicit loss policy. Interpret as re-decodes original bytes and confirms dirty-state consequences; Convert to changes the save target and refuses unresolved opaque bytes or unrepresentable characters unless explicitly resolved. Large transcodes spill to disk. See [FC-01/02](09_FOUNDATION_CONTRACTS.md).

### 3.8 Save algorithm

1. Capture revision, ContentStateId, complete generation, target identity and full fingerprint. Refuse Save/Save As when source bytes are unavailable.
2. Stage encoded output to a same-directory temporary file; flush and verify completion.
3. Preserve supported metadata and apply hard-link/reparse-point policy (FC-04).
4. Revalidate the target immediately before replacement; retain staged content on conflict. Do not claim atomic compare-and-swap against arbitrary external writers.
5. Replace through the platform capability contract; unsupported filesystems offer Save Copy.
6. Record saved ContentStateId after success. Newer edits remain dirty; undo to the saved state becomes clean.
7. Retire recovery data only after the save/manifest transition is durable.

See [FC-03/04](09_FOUNDATION_CONTRACTS.md) for failure, filesystem and metadata semantics.

### 3.9 Recovery journal

Own inserted and inverse bytes before commit. Flush segments before a checksummed journal record acknowledges a durable revision. Publish checkpoints through an atomic manifest only after referenced data is durable; cleanup follows reference retirement.

The first dirty edit starts a sealed recovery baseline in the background. Until ready, display 'Edits protected; full snapshot preparing'. Complete reconstruction requires that baseline or a verified original. Distinguish Complete, Edits only, Corrupt tail and Source unavailable; partial recovery export includes a gap report. See [FC-03](09_FOUNDATION_CONTRACTS.md).

### 3.10 Commands

Every action is registered as:

```rust
CommandSpec {
  id: "editor.find.next",
  title_key: "cmd.editor.find.next",
  category: CommandCategory::Search,
  default_shortcuts: [...],
  enabled: predicate,
  handler: command_fn,
}
```

Menus, toolbar, context menus, command palette, macro recorder and extension contributions all consume the registry. Localized labels never become persistence keys.

### 3.11 Extensions

`bareline.exe` never loads third-party DLLs. An optional `bareline-extension-host.exe` runs separately. Preferred package format is a signed/hashed `.blex` archive containing `manifest.toml` and a WebAssembly component. Host uses `wasmtime` with the component model and exposes a capability-limited API. Editor↔host communication is length-framed versioned `postcard` messages over a named pipe on Windows, handled by a dedicated blocking thread.

**The host is a separate download (ADR-20).** `wasmtime` is a JIT and weighs more than the entire core installer budget, so it is not bundled. On the first extension enable Bareline downloads the signed `bareline-exthost-x64` pack (also downloadable standalone for offline machines). With zero extensions enabled no host process runs; previously installed runtime files remain until explicit removal.

Capabilities include `document.read`, `document.edit`, `workspace.read`, `workspace.write`, `network`, `process.spawn`, `ui.panel`, `settings`. Dangerous capabilities require explicit user approval. The editor brokers document mutations as transactions so an extension never gets raw internal pointers.

First-party extensions (JSON tools, XML tools, Hex view; PR-027) live in `extensions/`, are licensed MIT OR Apache-2.0 like the SDK, and are the reference implementations for the documented SDK. The catalog index is JSON signed with minisign in a public `bareline-extensions` repository.

### 3.12 Platform abstraction

`PlatformServices` covers native menu bar and context menus, dialogs, clipboard, shell open, file metadata/watch details (an own `ReadDirectoryChangesW` wrapper on Windows), tray, process spawning, updater, single-instance IPC, printing, and accessibility hooks. Core crates take trait objects/generics; they never import `windows` crate directly.

### 3.13 Dependency policy (ADR-09)

The full approved list is in `07_DECISION_LOG.md`. The rules that matter most for speed and size:

- **No async runtime in the editor process.** `std::thread`, channels and `rayon` only. IPC and process pipes run on dedicated blocking threads.
- **No web view, no `egui`/`iced`/`gpui`, no `reqwest`/`openssl`, no `resvg`, no `image` beyond `png`, no `notify`.** Update checks use WinHTTP through `windows`.
- **Approved core set:** `windows`, `winit`, `rayon`, `crossbeam-channel`, `thiserror`, `anyhow` (binaries only), `tracing` family, `serde`, `toml_edit`, `serde_json`, `postcard`, `pcre2`, `memchr`, `aho-corasick`, `unicode-segmentation`, `unicode-width`, `encoding_rs`, `crc32c`, `sha2`, `sha1`, `md-5`, `base64`, `percent-encoding`, `lexopt`, `png`, `accesskit*`, `cxx`, `minisign-verify`, `zip`.
- **Host only:** `wasmtime`, `wasmtime-wasi`.
- Every new dependency needs a one-line justification in the PR and a passing `cargo deny`.

### 3.14 Startup budget (ADR-33)

Before the first frame the process may: parse the CLI (`lexopt`), read `settings.toml`, read the keymap, read the session manifest header, create the window and the renderer. It may **not**: restore tabs, read documents, enumerate fonts beyond resolving the editor font family, rasterize icons for hidden toolbars, touch the network, start the extension host, or stat recent files. Session tabs restore incrementally after the first frame, active tab first. A debug-build ledger records every I/O call before first frame and fails a test if the list grows.

### 3.15 Panic and crash policy (ADR-08)

Release builds use `panic = "abort"`. Recovery journals are appended on every edit interval, so an abort loses at most one interval. A crash handler writes version, build hash, panic location and renderer state to the diagnostics log and never document content. Debug builds unwind.

## 4. Concurrency model

The UI thread handles events, bounded dispatch and rendering. DocumentService is a logical actor on a bounded shared worker pool; 5,000 tabs do not create 5,000 threads. Start with two CPU workers, capped by available CPUs. I/O, search and watchers have bounded queues. Blocking IPC/watch threads sleep while idle. Cancellation and backpressure apply to every producer.

Use the shared budgets in [FC-02/06](09_FOUNDATION_CONTRACTS.md). Linked changes across open documents prepare inverse data and validate all revisions before committing roots in document-ID order. UndoGroupId prevents partial linked undo. Never hold a document lock across I/O or extension calls.

## 5. Main data contracts

### Document position

Use byte offsets into the internal UTF-8 sequence as canonical positions (ADR-02). Persisted positions (session, bookmarks) store the byte offset plus a line/column hint so they can be re-validated after the file changes on disk or its encoding is reinterpreted. UI exposes line/column and grapheme positions computed through indexes. Never store an editor position solely as a UTF-16 index.

### Edit transaction

```rust
EditTransaction {
  base_revision: Revision,
  edits: Vec<TextEdit>, // non-overlapping sorted byte ranges in base revision
  cursor_after: SelectionSet,
  source: EditSource,   // keyboard, macro, extension, replace-all...
}
```

Validate and normalize all ranges before mutation. Multi-cursor edits become one transaction and one undo record.

### Selection set

```rust
SelectionSet {
  primary: usize,
  ranges: SmallVec<[Selection; 4]>,
  shape: Stream | RectangleProjection,
}
```

Rectangle is converted to per-line selections at operation time using display-column mapping; after conversion, all commands use normal ranges.

### Render backend contract

The neutral UI/editor code depends on a narrow painter API, conceptually:

```rust
trait RenderBackend {
    fn begin_frame(&mut self, viewport: SizePx, scale: f64) -> Result<Frame>;
    fn fill_rect(&mut self, frame: &mut Frame, rect: RectPx, brush: BrushKey);
    fn stroke_line(&mut self, frame: &mut Frame, a: PointPx, b: PointPx, stroke: StrokeKey);
    fn push_clip(&mut self, frame: &mut Frame, rect: RectPx);
    fn pop_clip(&mut self, frame: &mut Frame);
    fn shape_text(&mut self, req: TextShapeRequest) -> Result<ShapedText>;
    fn draw_text(&mut self, frame: &mut Frame, text: &ShapedText, origin: PointPx, spans: &[TextPaintSpan]);
    fn end_frame(&mut self, frame: Frame) -> Result<()>;
}
```

The exact Rust signatures may evolve in PR-001/003, but the separation may not. Device-loss and resize handling are backend concerns. Text-layout cache keys must not contain platform COM pointers.

## 6. Settings layout

```text
%APPDATA%/Bareline/settings.toml            # TOML, human-edited, comment-preserving
%APPDATA%/Bareline/keymap.toml
%APPDATA%/Bareline/themes/*.toml
%APPDATA%/Bareline/macros/*.toml
%APPDATA%/Bareline/languages/*.toml         # UDL
%LOCALAPPDATA%/Bareline/state/session-vN.json   # JSON, machine-written
%LOCALAPPDATA%/Bareline/state/recovery-manifest.json
%LOCALAPPDATA%/Bareline/recovery/<id>.journal   # binary, CRC32C records
%LOCALAPPDATA%/Bareline/logs/
%LOCALAPPDATA%/Bareline/extensions/         # host pack + installed .blex, only if enabled
```

Format rule (ADR-21): TOML for anything a human edits, JSON for machine-written state, binary for journals. All versioned. Portable mode relocates all persistent locations under `./data/` and suppresses registry integration.

## 6A. File compare architecture

The diff engine is an OS-neutral crate `crates/diff` delivered by **PR-025 in wave 2** (ADR-12) so that PR-004's conflict dialog and recovery center, PR-015's external-change preview and PR-017's Compare Workspace all consume one engine. PR-017 builds the two-editor workspace on PR-010 split-view and synchronized-scrolling primitives rather than creating a second pane system. Inline (unified) view and folder compare are v1.1 (ADR-26).

### Core diff model

`crates/diff` exposes, conceptually:

```rust
CompareOptions {
  whitespace: Significant | TrimEdges | IgnoreAll,
  ignore_blank_lines: bool,
  ignore_case: bool,
  ignore_eol_style: bool,
  ignore_encoding_bom: bool,
  normalize_tabs: bool,
}

DiffKind = Equal | Added | Removed | Changed | MovedAligned
DiffHunk { left_text_bytes, right_text_bytes, left_line_hint, right_line_hint, kind, intraline_spans, stable_id }
CompareResult { left_revision, right_revision, left_generation, right_generation, hunk_batches, terminal_state, stats }
```

The concrete API may evolve, but the service must accept immutable document snapshots/revisions, execute off the UI thread, be cancellable, and publish results tagged with source revisions so stale results are never painted after edits.

### Algorithm and resource behavior

Use a bounded line-oriented diff suitable for source/text files (for example patience/histogram anchoring with Myers on bounded regions), followed by bounded intraline refinement on changed line pairs. Do not run quadratic worst-case work without limits. Large or highly divergent inputs must switch to a coarse block/anchor strategy and expose `CompareTerminal::CompletedCoarse` rather than freezing or exhausting memory. Comparison should stream/snapshot chunks and must not materialize multi-GB files into duplicate whole-file strings.

### Compare workspace UI

`CompareWorkspace` composes two normal `EditorView`s plus PR-010 synchronization/alignment infrastructure. Diff alignment spacers are view metadata only and never mutate either document. Navigation uses stable hunk IDs; edits invalidate only affected regions where the implementation can do so safely, otherwise schedule a debounced background recompare. Merge/copy operations are standard document edit transactions and therefore inherit undo/redo, dirty state, encoding, recovery, and save safety.

### Theme and decoration contract

Compare colors are semantic theme tokens, not hard-coded RGB values: `diff.added`, `diff.removed`, `diff.changed`, `diff.current`, `diff.moved`, plus gutter/overview/connector variants. Light, Dark and System themes each provide defaults; System resolves from current Windows app theme and updates live. User overrides are stored by semantic token and survive theme switching. Accessibility decorations include non-color gutter glyph/pattern distinctions.

### Conflict/recovery integration

File-write conflicts and crash-recovery previews call the same compare service with restricted actions. Conflict compare must never overwrite either version without an explicit command. Recovery compare can compare recovered content with current disk content.

### Compare-specific tests

- Golden diff fixtures for insert/delete/change/move-like alignment, Unicode, blank lines and all ignore options.
- With ignore options off, applying all hunks reproduces the target; with ignore options on, only selected nonignored ranges change. Both paths remain undoable.
- Stale-result test: edit during background compare and prove an old revision result is discarded.
- Multi-GB generated fixture verifies bounded memory, cancellation and coarse fallback.
- Theme tests verify semantic compare tokens resolve in Light/Dark/System and custom overrides persist.
- UI interaction tests verify next/previous difference, synchronized scroll, swap sides and merge actions.

## 7. Testing strategy

### Unit/property

- Piece-tree split/merge/newline counts.
- Random edit sequences compared with a simple reference string for files small enough to materialize.
- Encoding round trips.
- Search offsets and regex replacement captures.
- Rectangle/multicursor normalization.
- Session schema migration.
- Safe path/archive extraction.

### Integration

- Open/edit/save/reopen across encodings.
- Crash process mid-journal/mid-save and recover.
- External modification conflicts.
- Extension crash/hang/permission denial.
- Multi-instance handoff.
- Shell context/CLI.

### UI

Every UI test runs against `RecordingBackend` (ADR-07) on Windows, Linux and macOS CI: layout, hit-testing, focus order, keybinding dispatch, control golden outputs and semantic-tree snapshots. A small number of real-renderer screenshot tests cover main workflows on Windows. Do not make pixel-perfect image tests the only acceptance mechanism.

### Performance (ADR-10)

Create reproducible fixture generators rather than committing multi-GB files. `xtask perf` exists from PR-001 and measures cold launch and idle private bytes; PR-019 completes the scenario set (open sizes, first styled viewport, scroll, edit latency, search, result jump, save, tail). A nightly CI job runs it and appends JSON with machine, OS, build, renderer mode and settings metadata to `tests/perf/results/`; a regression above 10% opens a tracking issue. The Notepad++ side-by-side comparison is run on the owner's reference machine and published with the release. **No performance number blocks a merge.**

## 8. Coding rules for implementation agents

- A PR may not replace an agreed cross-platform core contract with a Windows-only shortcut unless the PR file explicitly permits it.
- No `unwrap()`/`expect()` on user-controlled file, encoding, session, extension or network/update data paths in production code.
- No blocking file/search/extension operation on the winit/UI event thread.
- No entire-file materialization before the first viewport is interactive, and never in RAM for files above the resident threshold (ADR-01).
- Only crates on the approved list (§3.13); each addition justified in the PR and passing `cargo deny`.
- Nothing runs before the first frame beyond the startup budget (§3.14).
- New user-visible actions must register a command ID from their own crate's `register_commands`.
- New persistent structures require schema/version and migration strategy, in the format prescribed by ADR-21.
- Every source file carries an SPDX header (`MPL-2.0` in core, `MIT OR Apache-2.0` in SDK and extensions).
- Every PR updates `03_PR_TRACKER.md` status fields only for its own PR and records verification evidence; performance measurements go in the evidence column and never change a status.
- Crate ownership: PR-002 owns `document`, PR-011 owns `commands`, PR-023 owns `ui` controls, PR-025 owns `diff`. Other PRs register or consume.

## 9. Build and quality commands

Canonical commands once scaffolded:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo test --workspace --locked
cargo test -p bareline-document
cargo test -p bareline-search
cargo test -p bareline-diff
cargo run -p bareline --release
cargo xtask perf --scenario launch,idle          # local perf snapshot, seconds
cargo xtask perf --all --compare notepadpp        # full comparison, owner machine
cargo xtask icons                                # rasterize SVG sources to sprite sheets
cargo xtask release --channel stable             # installer, portable zip, checksums, SBOM, notices
```

`xtask perf` is quick enough to run locally on any PR and runs nightly in CI; it is informational, never a merge gate.

## 10. PR execution model

The implementation is split into 27 focused files in `PRs/`. Each file repeats the architecture context required for that change, exact scope, interfaces, tests and acceptance criteria. Agents should read:

1. their assigned PR file;
2. `07_DECISION_LOG.md` whenever an architecture question arises; it overrides everything else;
3. the directly dependent PR file(s) only when an interface is unclear;
4. `03_PR_TRACKER.md` to update status/evidence.

They should **not** scan all requirement documents unless the PR explicitly says a product-level ambiguity exists.

See `04_PR_DEPENDENCIES_AND_PARALLELISM.md` for waves and parallel execution.

## 11. Release engineering summary

Full detail is in `06_OPEN_SOURCE_AND_DISTRIBUTION.md`. In short: MPL-2.0 core with MIT OR Apache-2.0 SDK; DCO sign-off; GitHub Actions CI on Windows, Linux and macOS; `cargo deny` and `cargo audit`; reproducible `--locked` builds from a pinned toolchain; Authenticode through SignPath Foundation or Azure Trusted Signing; minisign-signed manifests and checksums; Inno Setup per-user installer, portable ZIP and winget manifest; SBOM and third-party notices with every release; monthly minor releases.

## 12. Research references

- [Notepad++ User Manual — User Interface / feature categories](https://npp-user-manual.org/docs/user-interface/)
- [Notepad++ User Manual — Preferences / large-file restrictions](https://npp-user-manual.org/docs/preferences/)
- [Notepad++ official GitHub repository](https://github.com/notepad-plus-plus/notepad-plus-plus)
- [Notepad++ 8.x changelog](https://github.com/notepad-plus-plus/notepad-plus-plus/wiki/Changes)
- [Notepad++ release 8.9.8 announcement](https://community.notepad-plus-plus.org/topic/27639/notepad-release-8-9-8)
- [Notepad++ Community — large-file architecture discussion](https://community.notepad-plus-plus.org/topic/21102/problem-with-big-files-50-mb/2)
- [Notepad++ issue #16922 — search result navigation freeze on ~50MB file](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/16922)
- [Notepad++ issue #17510 — pinned tabs lost after update](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/17510)
- [Notepad++ issue #18062 — session save error in synced folder](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/18062)
- [Notepad++ issue #18355 — column paste bug in 8.9.8](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/18355)
- [Notepad++ FAQ — plugin incompatibility after host changes](https://community.notepad-plus-plus.org/topic/23146/faq-notepad-crashes-freezes-unresponsive-after-update)
- [winit 0.30 desktop window/event layer](https://docs.rs/winit/latest/winit/)
- [winit 0.30.13 release](https://github.com/rust-windowing/winit/releases/tag/v0.30.13)
- [Microsoft Direct2D overview](https://learn.microsoft.com/en-us/windows/win32/direct2d/direct2d-overview)
- [Microsoft DirectWrite overview](https://learn.microsoft.com/en-us/windows/win32/directwrite/introducing-directwrite)
- [Direct2D + DirectWrite text rendering](https://learn.microsoft.com/en-us/windows/win32/direct2d/direct2d-and-directwrite)

## v1.2 implementation contracts

[Foundation contracts](09_FOUNDATION_CONTRACTS.md) define exact budgets, bounded diff output and terminal states, extension framing, signing order and benchmark methodology. [Acceptance cases](10_ACCEPTANCE_AND_TRACEABILITY.md) assign evidence. [Interaction specification](11_UI_INTERACTION_SPEC.md) defines visible state transitions. Code examples describe interfaces, not existing Rust implementation.
