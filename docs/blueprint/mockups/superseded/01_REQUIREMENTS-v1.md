# Bareline Product Requirements Document

**Version:** 1.2  
**Date:** 2026-09-05  
**Status:** implementation-ready baseline (decisions recorded in `07_DECISION_LOG.md`, which overrides any conflicting sentence here)  
**Tagline:** *Plain text. Full power. No weight.*  
**License:** MPL-2.0 core, MIT OR Apache-2.0 extension SDK; name and logo trademarked (see `06_OPEN_SOURCE_AND_DISTRIBUTION.md`)

## 1. Product definition

Bareline is a Windows-first, offline-first, open-source, lightweight native text and code editor that provides feature parity with the productive built-in capabilities of Notepad++ while deliberately fixing its most consequential architectural and UX weaknesses. It is **not** a full IDE and must not become one by default.

The product is successful when a Notepad++ power user can move to Bareline without losing daily editing capabilities, while gaining a measurably faster launch, lighter idle footprint and faster file opening on the same machine, safer recovery, cleaner UX, consistent multi-editing, much better huge-file behavior, safer extensions/updates and a path to Linux/macOS.

Three owner constraints shape every requirement below:

1. **Faster, lighter, loads better than Notepad++**, proven by published side-by-side measurements.
2. **No developer-blocking gates.** Performance numbers are targets that are measured continuously; they never block a PR.
3. **Open source, owned and distributed by the product owner.**

## 2. Scope

### 2.1 In scope for the first complete Windows release

- Windows 10 and Windows 11 on x64.
- Native installer and portable ZIP.
- Text files from zero bytes through multi-gigabyte files.
- Plain text, source code, structured text, logs and configuration files.
- All core Notepad++ productivity feature families: file operations, tabs/views, editing, search/replace, regex, bookmarks, encodings/EOL, syntax/folding, UDL, auto-completion, sessions/workspaces, function/document panels, macros, external commands, plugins/extensions, preferences/themes/shortcuts, command line, shell integration, print/export, hashes, compare, update, tray behavior and diagnostics.
- Three first-party extensions (JSON tools, XML tools, Hex view) that also serve as the SDK reference.
- Stable platform abstraction so Linux/macOS ports reuse the same core.
- Public source repository, reproducible unsigned payloads and signed distribution artifacts, SBOM and third-party notices with every release.

### 2.2 Deliberately out of default scope

- Built-in AI assistant.
- Git client, debugger, compiler, package manager, database browser or integrated terminal as permanent core panels.
- Cloud account/sync service.
- Collaborative editing.
- Full LSP IDE semantics by default. Extensions may add optional LSP support later without bloating the core.
- Executing untrusted code merely because a text file was opened.
- Telemetry of any kind.

### 2.3 Deferred to v1.1 (decided, not forgotten)

- Per-file local history (ADR-24).
- Inline (unified) compare view and folder compare (ADR-26).
- Character Panel, Post-It mode, Document Peeker (ADR-22).
- MSIX package for the Microsoft Store (ADR-31).
- Memory-mapped `ByteSource` as an optional third storage mode (ADR-01).
- Hex editing (viewer ships in v1 through PR-027).

## 3. Product principles

1. **Text first:** launch to an editor, not a home dashboard.
2. **Fast by architecture:** no browser runtime, no async runtime in the editor process, no whole-file mutable buffer, no eager indexing of the world, nothing but settings, keymap and session header read before the first frame.
3. **Never block the UI thread:** file I/O, regex across large data, syntax, workspace search, hashing, update checks and extension calls must be asynchronous/cancellable.
4. **Data safety over cleverness:** atomic saves and recoverable journals beat magical autosave.
5. **Power is searchable:** every command has a stable command ID and can appear in menus, shortcuts and the command palette.
6. **Extensions are optional processes:** zero extension cost when none are enabled, including zero runtime on disk.
7. **Cross-platform core, platform-native edges:** Windows integration is first-class without leaking Win32 assumptions into the core model. Native menus and dialogs are used where they are free.
8. **Performance is designed in, measured always, gated never:** every PR inherits the startup budget and dependency policy; `xtask perf` runs nightly; numbers are published, not used to block merges.
9. **Open by default:** public code, DCO contributions, signed releases with reproducible unsigned payloads, and a trademark policy that protects the name while inviting forks.

## 4. Target users

- Developers and system administrators who use Notepad++ for quick code/config/log edits.
- Analysts/support engineers who search/transform large text, CSV, SQL, XML, JSON and logs.
- General users needing a reliable replacement for basic Notepad with stronger editing.
- Power users who depend on regex, macros, column editing, multiple tabs, syntax coloring and plugins but do not want VS Code-class weight.

## 5. UX requirements

### 5.1 Main window

The main window must remain visually quiet:

- Title bar and a native menu row at top (File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help). The menu is a projection of the command registry (ADR-04).
- Optional compact toolbar under it; **hidden by default**. Users who want it turn it on once.
- Tabs directly above the editor.
- Editor consumes at least 80% of the main-window area in the default layout.
- Left sidebar is hidden by default; toggled for Workspace/Documents.
- Right sidebar is hidden by default; toggled for Outline/Document Map/Properties.
- Bottom panel is hidden until Find in Files, output/task, diagnostics or extension output is requested.
- Status bar is one line and shows language, size/lines, cursor/selection, EOL, encoding and insert/overwrite status.

### 5.2 Command palette

`Ctrl+Shift+P` opens a centered palette. It searches command name, synonyms, menu path and assigned shortcut. Actions execute without forcing users to remember which menu contains them.

### 5.3 Search experience

Use one search surface with tabs/scopes instead of multiple unrelated dialogs:

- Find
- Replace
- Find in Open Documents
- Find in Folder/Workspace
- Mark/Bookmark

The compact in-editor find bar opens with `Ctrl+F`; `Ctrl+H` expands to replace. `Ctrl+Shift+F` opens the full search panel.

### 5.4 Settings

One searchable settings window with categories. Advanced values exist but are collapsed by default. Every setting displays its key so power users can edit `settings.toml` if desired.

## 6. Functional requirements

### FR-001 New/Open/Recent/Drag-drop

- Create untitled documents instantly.
- Open one or many local files.
- Open folders/workspaces.
- Drag files/folders onto the window.
- Recent files and recent workspaces with pin/remove/clear actions.
- Detect files changed/deleted externally; explicit **Reload From Disk** command.
- Configurable handling for files opened from shell, CLI or another instance.
- Resolve symlinks/reparse points safely and display actual path/trust state.
- Files up to 256 MiB (configurable) load fully into memory in a streaming pass with the first viewport shown immediately; larger files are paged on demand and never fully materialized (ADR-01).
- Restore Last Closed Tab (`Ctrl+Shift+T`) with caret and pin state.

### FR-002 Save family

- Save, Save As, Save a Copy, Save All.
- Atomic replace when possible; never truncate the original before a successful new version exists.
- Preserve encoding/BOM/EOL by default.
- Preserve relevant file attributes/ACL metadata when replacing.
- Detect write conflicts and present Reload / Compare / Overwrite / Save As.
- Optional safe backup-on-save policy.

### FR-003 Tabs and document management

- Horizontal tabs, optional vertical tabs.
- Pin/unpin.
- Reorder by drag and keyboard.
- Close, close others, close left/right, close unchanged, close all but pinned.
- Sort by name/path/type/size/modified time.
- Rename untitled display name without pretending it is saved.
- Rename actual files.
- Move to recycle bin with confirmation policy.
- Copy filename/path/folder path.
- Per-document **Set Read-Only** toggle (editor lock, independent of the file attribute).
- MRU document switcher (`Ctrl+Tab`).
- Document list for very large tab counts.
- Tab model, pin state and persistence are one component; the tab strip UI is another (ADR-14).

### FR-004 Split views

- Two-pane split horizontal or vertical.
- Move or clone document to other view.
- Same document may be viewed twice with independent cursors/scroll positions.
- Optional synchronized vertical/horizontal scrolling.
- Drag splitter; restore layout with session.

### FR-005 Core text editing

- Unicode-correct insertion/deletion.
- Undo/redo with bounded configurable history and transaction grouping.
- Cut/copy/paste, line copy/cut with no selection option.
- Select all; word/line/paragraph selections.
- Insert vs overwrite mode.
- Smart Home/End behavior.
- Duplicate line/selection, move line up/down, join lines, split lines.
- Indent/unindent; tabs↔spaces.
- Trim leading/trailing/both whitespace.
- remove empty/duplicate/consecutive duplicate lines; sort lines; randomize if compatibility requires it.
- Case transforms: upper/lower/title/sentence/invert/random where parity commands exist.
- Comment/uncomment/toggle block/line comments by active language.
- Transpose character/word/line where useful.
- **Paste Special** (plain text without formatting) and drag-and-drop text move within and between views.
- **Hide Lines / Show Hidden Lines** as view-only folding of arbitrary selections; hidden lines never change bytes or save output.
- Optional **clipboard history**: off by default, in-memory only, 20 entries, cleared on exit, never persisted (ADR-23).

### FR-006 Multiple cursors and rectangular editing

- `Ctrl+Click` add/remove caret.
- Select next/all occurrence.
- Add caret above/below.
- Rectangle selection via Alt+drag and keyboard.
- Rectangle typing, delete, paste, indent and navigation use the same normalized multi-selection engine.
- **Column Editor** (`Alt+C`): insert a number sequence across the rectangle with start, step, zero padding and base 10/16/8/2, or insert repeated text.
- Operations are Unicode-safe and must never split a UTF-8 sequence or grapheme unexpectedly.
- One undo action restores an entire multi-cursor transaction.

### FR-007 Search and replace

- Find next/previous; wrap option.
- Whole word, case sensitive, literal, extended escape and regex modes.
- PCRE2-compatible regular expressions including captures/backreferences/lookarounds where supported by PCRE2.
- Replace one, replace all, replace in selection, replace in open docs, replace in workspace.
- Count matches.
- Mark all; bookmark matching lines. **Five independent mark styles** (like Notepad++ "Style token 1–5"): mark with style N, clear style N, clear all; marks are per view, never written to the file.
- Regex uses bounded UTF-8 windows and a context-preserving PCRE2 adapter. Unsupported streaming patterns, cancellation and limits are visible; incomplete results cannot drive replacement (FC-05).
- Search history with privacy-aware clear controls.
- Incremental/as-you-type search.
- Results grouped by file with line/column, excerpt and expandable matches.
- Clicking a result must be fast even in huge files because navigation is by byte offset/index, not a full reparse.
- Long searches are cancellable and never freeze typing/scrolling.

### FR-008 Navigation/bookmarks

- Go to line, column, byte/character offset.
- Brace/bracket matching and jump.
- Bookmark toggle, next/previous, clear, cut/copy/delete bookmarked lines.
- Jump history back/forward.
- Optional persistent bookmarks per workspace.

### FR-009 Encodings and EOL

- UTF-8, UTF-8 BOM, UTF-16 LE/BE, UTF-32 and common Windows/ISO/East Asian code pages.
- **Text and raw bytes have separate domains.** The valid UTF-8 text view retains original-byte provenance, including tagged invalid spans. Unchanged same-encoding spans copy original bytes; large transcodes are disk-backed (FC-01/02).
- Best-effort encoding detection with visible confidence; never silently reinterpret a file after edit.
- “Interpret as” re-runs the open transcode from the original bytes; “Convert to” changes only the save target. They are distinct actions.
- Lossy conversions are refused by default with the offending characters listed; replacement is an explicit user choice.
- EOL recognition for CRLF/LF/CR; convert entire file or selection.
- Preserve mixed EOL unless user asks to normalize.
- Unicode normalization actions optional but explicit.
- Binary/NUL-heavy detection warns before treating binary as text; the first-party Hex view extension handles binary.

### FR-010 Syntax highlighting and language detection

- Syntax highlighting for at least the language families available in current Notepad++ through Lexilla-compatible lexers/language packs where licensing permits.
- Extension/name/shebang/modeline detection.
- Manual language override per document.
- Folding, matching braces and comment tokens. Commands: Fold All, Unfold All, Fold Level 1–8, Toggle Current Fold.
- Large files highlight viewport-first and asynchronously. Lexilla is the primary lexer; the native UDL engine doubles as a fallback lexer, and any language whose Lexilla path cannot style the first viewport of a 1 GB file within 100 ms defaults to its native definition (ADR-17).
- Theme styles configurable per token category using the tokens in `00_BRAND_AND_LOGO.md`.

### FR-011 User-defined languages

- Create/edit custom language definitions: extensions, keywords, operators, comments, strings, numbers, delimiters, folding and styles.
- Import/export Bareline language definitions.
- Import common Notepad++ UDL XML into Bareline schema when fields can be mapped; report unsupported semantics rather than silently dropping them.
- Live preview against a sample document.

### FR-012 Auto-completion and smart typing

- Word completion from current document and optionally open documents.
- Language keyword completion.
- Function/parameter hint definitions where language data provides them.
- Auto-close quotes/brackets/braces with configurable per-language behavior.
- Smart indentation based on language rules.
- Completion must be local/offline and instant; no AI dependency.

### FR-013 Views and visual aids

- Word wrap modes.
- Show whitespace, tabs, EOL, control characters.
- Indent guides.
- Configurable edge/column markers and background zones.
- Zoom in/out/reset.
- Line numbers; relative line numbers optional.
- Current line highlight.
- Long-line indicators.
- Fullscreen/distraction-free mode.
- Always-on-top option.
- Smooth scrolling optional; scrolling must remain precise for huge files.

### FR-014 Document Map / minimap

- Optional document map with visible viewport marker.
- For huge files, generate coarse map progressively; map must never require full syntax before display.
- Click/drag to navigate.

### FR-015 Function/outline list

- Outline panel for functions/classes/sections when lexer/parser definitions support it.
- Import Notepad++ `functionList` parser definitions where mappable, with an imported/approximated/unsupported report.
- Search/filter outline.
- Follow caret option.
- Navigate without blocking if the rest of the file has not been indexed; progressively fill.

### FR-016 Workspace/folder explorer

- Open folder as workspace.
- File tree with filter, refresh, create/rename/delete file/folder.
- Respect hidden/system file preferences.
- Multiple root folders optional but supported in the model.
- Workspace-specific settings stored in `.bareline/settings.toml` only when the user enables workspace settings; never create project files merely by opening a folder.
- Find in workspace and function/document panels integrate with workspace.

### FR-017 Sessions and recovery

- Restore windows, split layout, tabs, pinned state, active file, caret/selection, scroll, sidebar/panel state and workspace roots.
- Named session save/load/export/import.
- Crash recovery journal for unsaved changes with checksums and timestamps.
- Recovery center on abnormal exit shows each recoverable buffer, original path if any, timestamp and diff/preview.
- User can recover, save as, discard or keep for later.
- Periodic recovery never mutates the user's actual file.
- Session state uses atomic versioned schema migrations. Session and recovery manifests are JSON; journals are binary with CRC32C (ADR-21).
- Session restore never delays the first frame: the window appears, then the active tab loads, then the remaining tabs restore incrementally (ADR-33).
- Per-file local history is v1.1 (ADR-24) and will be kept distinct from recovery.

### FR-018 File monitoring / tail

- “Follow file” mode equivalent to `tail -f` for logs.
- Efficient append detection.
- Pause/resume follow when user scrolls away from end.
- Rotation/truncation detection.
- No accidental edit/save while in monitoring mode unless explicitly unlocked.

### FR-019 Macros

- Record editor/command actions with stable command IDs.
- Stop, play once, run N times, run until EOF.
- Save, rename, assign shortcut, export/import.
- Macro format is human-readable and versioned.
- Input-requiring or dangerous actions are recorded only when deterministic and safe.
- Include an optional “type/replay text with delay” command to cover the lightweight ghost-typing/replay use case without making it a separate subsystem.

### FR-020 External commands

- Define named external commands with placeholders for file, directory, selection, line and column.
- Run in hidden or visible process mode.
- Capture stdout/stderr to an optional bottom output panel.
- Explicit confirmation if a command interpolates unsaved selection/text into a shell.
- No shell invocation when direct process argument execution is possible.

### FR-021 Built-in utilities

- Hash selected text or file with MD5, SHA-1, SHA-256, SHA-512 for parity; clearly label MD5/SHA-1 as legacy integrity tools, not secure signing.
- Base64 encode/decode and URL encode/decode.
- Character/byte/line statistics.
- Export syntax-colored selection/document to HTML/RTF through an optional utility module.
- Print current document/selection with header/footer and line-number options.

### FR-021A File Compare Workspace

File comparison is a first-class built-in Bareline workflow, not an extension-only feature. The diff engine is an OS-neutral core crate delivered early (PR-025) so conflict dialogs and the recovery center reuse it; the Compare Workspace UI arrives with PR-017. Inline (unified) view and folder compare are v1.1 (ADR-26).

- Open any two files, open documents, or saved snapshots in a dedicated side-by-side compare workspace.
- Support vertical side-by-side as the default and horizontal top/bottom comparison as an alternate layout.
- Keep both panes independently editable unless the user enables read-only compare mode.
- Compute line-level differences and refine changed lines to word/token/grapheme-level highlights when practical.
- Visually distinguish **Added**, **Removed**, **Changed**, **Moved/Aligned**, and **Current Difference** states.
- Difference colors are configurable independently for Light, Dark, and System themes. Each state supports background, foreground/accent, gutter marker, overview-ruler marker, and connector-line color where applicable.
- System theme follows Windows light/dark mode automatically while retaining user compare-color overrides unless the user selects “Use theme defaults.”
- Provide color-blind-friendly built-in compare palettes and never rely on color alone: gutter symbols/patterns and labels must also communicate state.
- Synchronized vertical scrolling is enabled by default with alignment spacers so corresponding differences remain visually aligned. Horizontal scroll synchronization is optional.
- Provide **Previous Difference**, **Next Difference**, **First/Last Difference**, and a compact difference counter such as `4 / 27`.
- Provide a collapsible difference navigator/overview strip with clickable changed regions.
- Support **Copy Left → Right**, **Copy Right → Left**, **Accept Left**, **Accept Right**, and selection/range copy actions. Every merge/copy operation is a normal undoable editor transaction.
- Never silently save either side after merge operations. Modified state remains visible until the user saves explicitly.
- Comparison options: ignore leading/trailing whitespace, ignore all whitespace, ignore blank lines, ignore case, ignore EOL style, ignore encoding/BOM metadata, and optional normalize-tabs-for-comparison.
- Recompare automatically after edits using debounced incremental work; provide a manual Recompare command and a pause-auto-recompare option for extremely large files.
- Allow comparing current file against: another open file, a file on disk, the last saved version, a recovery snapshot, or an externally modified disk version when resolving conflicts.
- For files too large or too dissimilar for detailed diff within configured resource limits, fall back gracefully to bounded-memory coarse line/block comparison while keeping the UI responsive and cancellable.
- Compare must work with UTF-8/UTF-16 and other Bareline-supported decoded text encodings without corrupting source files. Binary files are out of scope for the text compare workspace and should offer a clear message/extension handoff.
- The compare workspace participates in session restore, remembering the two sources, layout, active side, sync-scroll choice, comparison options, and theme-specific display settings.
- Commands are accessible from `Tools > Compare`, tab context menus, command palette, and conflict/recovery dialogs.

**Compare screen UX:** two equal editor panes with independent tab headers; central divider with optional directional merge controls; diff gutters on both sides; aligned changed blocks; minimap/overview difference markers; a compact top compare toolbar for source labels, swap sides, options, previous/next, recompare, and close compare. Avoid a heavy IDE-style merge dashboard.

### FR-022 Extensions/plugins

- Extension manager: Installed / Discover / Updates / Disabled, with permission chips per extension and “Runs isolated” copy.
- Extensions never load code into `bareline.exe`.
- Versioned manifest, declared capabilities and host API version.
- Separate extension-host process (`wasmtime` component runtime) starts only when needed. The host runtime is **not bundled** in the core installer; it is a signed optional download fetched on first enable, also available standalone for offline installs (ADR-20). With zero extensions enabled the host stops; installed runtime files remain until explicit removal.
- Crash/timeout isolation and per-extension disable-on-crash option.
- Signed catalog metadata (minisign) and package hashes; catalog index lives in a public `bareline-extensions` repository reviewed by pull request; local sideload supported with warnings.
- **First-party extensions shipped at v1:** JSON tools, XML tools, Hex view (PR-027). They are the SDK reference implementations and are licensed MIT OR Apache-2.0.
- Further extension examples: advanced CSV, LSP client, spell check.
- Stable API areas: commands, menus, panels, document read/edit transactions, selections, workspace, notifications, settings, decorations, search providers.
- No unrestricted raw pointer/window-message contract.

### FR-023 Preferences, shortcuts and themes

- Searchable settings UI; every row shows its TOML key with a copy action.
- Global `settings.toml`, with versioned schema and comments-preserving editing through `toml_edit`. Human-edited files (settings, themes, keymaps, macros, UDL, language catalog) are TOML; machine-written state is JSON (ADR-21).
- Shortcut mapper supports conflicts, multi-chord optional, import/export.
- Every built-in action has immutable command ID separate from localized label.
- Light, Dark and System themes; high-contrast support.
- Custom editor theme files.
- Toolbar visibility/customization.
- Per-language indentation settings.

### FR-024 Localization

- UI strings externalized. English ships in v1 with the complete resource system; additional language packs are accepted as community contributions through the open-source repository (ADR-28).
- Runtime language switching where toolkit permits, otherwise restart prompt.
- Right-to-left UI readiness and BiDi text correctness in editor where underlying shaping supports it.
- Language packs do not change command IDs/config keys.

### FR-025 Command line and instance behavior

`bareline [options] [files...]` must support:

- file/folder paths;
- line/column jump;
- read-only;
- monitoring/follow;
- new instance / reuse instance;
- no-session / safe mode / no-extensions;
- portable config location;
- open workspace;
- encoding/language override;
- print and command execution where safe;
- stdin (`-`) into a new untitled document without blocking UI startup.

### FR-026 Windows shell integration

- Optional “Edit with Bareline” Explorer context menu.
- Optional file associations selected by user/installer; never steal defaults silently.
- “Open containing folder” and terminal/cmd actions.
- Jump list/recent files integration if it does not harm startup.
- System tray modes: minimize, close, both, or never.
- Single-instance handoff through a named pipe/IPC protocol without corrupting session state.

### FR-027 Update and diagnostics

- Check manually or on configurable schedule.
- Signed update manifest; verify package hash and Authenticode/code signature before staging.
- Stage update to a private temp directory with safe extraction; apply after exit; rollback on failure.
- No update requires editor to run elevated continuously.
- “About / Diagnostics” copies version, renderer, OS, architecture, enabled extensions, config locations and sanitized performance info.
- Security log records blocked unsafe paths/packages without including document content.

### FR-028 Portable mode

- Portable ZIP runs without installer.
- A marker/config switch keeps all settings, sessions, recovery and extensions under the portable folder, except OS temp files that are securely deleted.
- Never write registry/file associations in portable mode.

### FR-029 Security and privacy

- Offline by default; no telemetry, crash upload or account.
- Never upload document contents.
- Treat UNC/network paths as potentially credential-leaking: prompt/trust policy before unsolicited remote access from session files, extensions or links.
- Safe archive extraction blocks `..`, absolute paths, symlink/reparse escape.
- Extension permissions are visible and revocable.
- Recovery data is stored with user-only ACLs.
- Clipboard remains OS-managed. The optional clipboard history (FR-005) is off by default, in-memory only, capped at 20 entries and never written to disk.
- Update checks use the OS HTTP stack, send no identifiers beyond the version string, and can be disabled entirely.

### FR-030 Accessibility and international input

- Full keyboard navigation of chrome.
- Screen-reader labels for menus, tabs, settings and panels.
- High-contrast and system scaling support from 100% through at least 300%.
- IME composition for CJK and complex scripts.
- Editor shaping must handle combining characters, emoji sequences and BiDi text without corrupting byte offsets.

## 7. Performance requirements

### 7.1 Policy (ADR-10)

- **Product targets** are comparisons against current stable Notepad++ with plugins disabled, on the same machine, same files, same settings. They define what “faster and lighter” means in public claims.
- **Engineering budgets** are absolute numbers every PR designs against. They exist so agents do not need a Notepad++ install to know whether a design is acceptable.
- **Measurement is continuous, not terminal.** PR-001 ships the `xtask perf` skeleton; a nightly CI job records launch time and idle memory from the first wave and appends JSON to `tests/perf/results/`. A regression above 10% opens a tracking issue automatically. PR-019 completes the scenario set and does the profiling pass.
- **No number is a merge gate.** A PR is blocked only by correctness, tests, lint and the architecture rules that make the numbers possible: no full-file materialization before first viewport, no UI-thread I/O, no forbidden dependencies, startup budget respected.
- **Release readiness** is a published table with raw data, machine metadata and methodology. README and site claims must match that table.

### 7.2 Targets and budgets

| Metric | Product target (vs Notepad++ P50, same machine) | Engineering budget |
|---|---|---|
| Empty cold launch to interactive editor | <= 0.8× | <= 250 ms P50 on reference SSD machine |
| Warm launch | <= 0.8× | <= 120 ms P50 |
| Idle CPU after settling | equal | ~0%, no timer waking every frame, caret blink pauses when unfocused |
| Idle private bytes, empty session | <= 1.0× | <= 25 MB x64 after settling; working set reported, not targeted |
| Open 10 MB UTF-8 file to first interactive viewport | <= 0.8× | <= 100 ms P50 |
| Open 100 MB UTF-8 file to first interactive viewport | <= 0.5× | <= 300 ms P50 (resident streaming load, viewport first) |
| Open 1 GB log to first interactive viewport | Notepad++ does not complete comparably | <= 1.0 s; paged, no full-file copy or parse |
| Open 5 GB log | Notepad++ does not complete comparably | supported on 64-bit Windows; UI interactive during indexing |
| Time to first styled viewport after open | <= 0.8× | <= 100 ms after first paint for top-15 languages |
| Typing/edit-to-paint latency | <= 1.0× | <= 8 ms P95 at 60 Hz workload, excluding OS compositor |
| Scroll frame time | <= 1.0× | <= 16.7 ms P95; no >100 ms main-thread stall from background work |
| Literal search throughput, 1 GB | >= 1.5× | >= 1 GB/s on reference machine |
| Find cancellation | n/a | visible stop within 50 ms of cancel |
| Search navigation in a 1 GB file | n/a | jump to known result <= 50 ms P95 after emission |
| Save 100 MB file | <= 1.0× | streamed, no whole-document string |
| Recovery durability | n/a | at most one recovery interval of unsaved work lost; default 5 s or 256 KiB of edits |
| Core installer size | <= 1.0× | <= 15 MB x64 excluding the optional extension host pack |

### 7.3 Design rules that make the budgets reachable

1. Startup budget (ADR-33): only settings, keymap and the session manifest header are read before the first frame. No font enumeration beyond the editor family, no icon rasterization for hidden toolbars, no network, no extension host, no document reads.
2. Dependency policy (ADR-09): no async runtime, no web view, no SVG renderer, no crate that starts threads at load time.
3. Renderer mode (ADR-32): both Direct2D hardware and software modes exist; the lighter one becomes the default from measurement.
4. Storage (ADR-01): files up to 256 MiB stream into memory viewport-first; larger files page on demand.
5. Release profile: `panic = "abort"`, fat LTO, one codegen unit, symbols stripped to a separate PDB.

### FR-031 Release integrity and openness

- Every release ships: signed installer and portable ZIP, `SHA-256SUMS` with a minisign signature, SBOM (CycloneDX), third-party notices, release notes, known issues and the performance table from 7.1.
- Builds are reproducible from a pinned toolchain and lockfile; two clean builds of the same commit produce identical hashes or the nondeterminism is documented.
- `cargo deny` and `cargo audit` pass in CI for licenses and advisories.
- About / Diagnostics shows the license, the commit hash and links to the repository and security policy.

## 8. Reliability requirements

- All destructive file operations require deterministic error handling.
- Save tests include disk-full, permission denied, file disappeared, antivirus lock, network disconnect and rename failure.
- Recovery journal fuzz/property tests ensure corrupt/truncated tail records do not invalidate previous records.
- No extension crash can terminate the editor process.
- Settings/session schema migration must be tested from every previously released schema version.
- Every background worker supports cancellation or bounded work units.

## 9. Cross-platform readiness requirements

- `bareline-core`, `bareline-document`, `bareline-search`, `bareline-syntax`, `bareline-session` and extension protocol crates contain no Win32 imports.
- All OS functions are behind `PlatformServices` traits.
- Paths remain `PathBuf`/OS strings; do not assume UTF-8 file paths.
- Shortcut display maps Control/Alt/Super per platform.
- Menus and shell integration are adapters.
- CI compiles core crates on Windows, Linux and macOS from the beginning even though only Windows GUI artifacts ship initially.

## 10. UI mockups and brand imagery

One screen per image, never a board. Every prompt repeats the design tokens from `00_BRAND_AND_LOGO.md`. The full prompt set, the shared style prefix and the review checklist are in `08_IMAGEN_PROMPTS.md`. The existing boards (`All_Screens.png`, `Comparison.png`) are marketing collateral only and must not be used as UI references (ADR-30). Exact interaction requirements above override any generated visual text.

| ID | Screen | Must show | Must not show |
|---|---|---|---|
| A1 | Main editor, dark | six tabs, one pinned, editor >= 80% of window, line numbers, one-line status bar with language, size/lines, Ln/Col, EOL, encoding, INS | sidebars, toolbar, minimap, marketing copy |
| A2 | Main editor, light | same as A1 in warm white | same |
| B | Workspace + split | 220 px folder tree, code file left, log file right, 240 px outline, discreet sync-scroll icon | toolbar, terminal, source control |
| C1 | Inline find bar | compact bar docked above the editor with regex/case/word toggles and match count | a dialog |
| C2 | Search panel | bottom panel with Find / Replace / Files tabs, grouped results by file with expandable matches | a dialog |
| D1 | Settings | left categories, search box, rows showing the TOML key, advanced collapsed | XML, Auto Save, Terminal categories |
| D2 | Extensions | Installed / Discover / Updates / Disabled, permission chips, “Runs isolated” copy, runtime-download state | the word “Plugins” |
| E | Huge log | 3.8 GB file editing normally, status “3.8 GB · indexing in background”, results streaming | “Large File Mode”, “Read-only” |
| F1 | Compare workspace, dark | two panes, gutters both sides, five diff states with glyphs, overview strip, counter “4 / 27”, prev/next, swap, options | inline view, folder tree |
| F2 | Compare options | Colors tab with semantic tokens for Light/Dark/System, color-blind palette toggle, ignore options | |
| G | Recovery Center | list of recoverable buffers with path, age, size, diff preview, Recover / Save As / Discard / Keep | |
| H | Command palette | centered palette over A1 with fuzzy results, shortcuts and menu paths | |
| I | Tail mode | log following with “Following · paused (scrolled up)” indicator and lock icon | |
| L1 | Logo mark | monoline lowercase b with fold and notch, cyan-teal accent | leaf, chameleon, pencil, quill |
| L2 | App icon set | 16 to 256 px tiles, dark and light | |
| L3 | Wordmark | “Bareline” in system UI weight with the mark | |

## 11. Acceptance definition for the product

A Windows v1 is product-complete when the tracker marks every PR **Accepted**, the Notepad++ parity matrix records a delivery route for every row and independent implementation/verification evidence for every included capability, data-loss/recovery tests pass, the performance table from section 7 is published with raw data against Notepad++ on the same machine, the release artifacts of FR-031 are present and signed, and the normal editor remains simple with optional power hidden until requested. No performance number by itself blocks acceptance; the published table does the talking.

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

## 13. v1.2 acceptance clarifications

- [Foundation contracts](09_FOUNDATION_CONTRACTS.md) are normative for byte fidelity, recovery completeness, save conflicts, long-line regex, linked undo, diff budgets, extension isolation and release trust.
- Unavailable source regions disable normal Save/Save As. Export available data includes a gap report.
- Recovery reports last durable edit and baseline readiness; it cannot recover bytes never owned.
- Windows compatibility floor: Windows 10 22H2 build 19045 and Windows 11 23H2 build 22631, x64. Verify those builds; compatibility does not imply vendor support entitlement.
- Named language pairs and codec coverage are in FC-01/09. English v1 and three first-party extensions do not establish localization or ecosystem breadth parity.
- [Acceptance and traceability](10_ACCEPTANCE_AND_TRACEABILITY.md) owns scenario IDs and evidence; all implementation remains pending.
- [Interaction specification](11_UI_INTERACTION_SPEC.md) controls keyboard, progress, error and destructive-action states. Images remain conceptual.
