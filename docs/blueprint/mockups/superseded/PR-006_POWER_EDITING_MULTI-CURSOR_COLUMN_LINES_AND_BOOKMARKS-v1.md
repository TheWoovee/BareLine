# PR-006 — Power Editing: Multi-Cursor, Column, Lines and Bookmarks

**Tracker row:** `PR-006` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-003`, `PR-023`  
**Primary objective:** Complete Notepad++-class editing commands on top of the unified selection model, including robust rectangular editing, the Column Editor, Hide Lines, Paste Special, drag-and-drop text move, optional in-memory clipboard history and bookmark workflows.

## Agent contract

This brief includes local scope and must be read with its assigned v1.2 foundation sections and acceptance cases below. Implement the scope below without scanning the whole documentation directory. Read a dependency PR only when you need the exact merged interface. If a requirement here conflicts with code already merged, preserve the public product intent and make the smallest compatible contract adjustment rather than inventing a parallel architecture. If an architecture question arises, read `../07_DECISION_LOG.md`; it overrides this file.

## Architecture snapshot (do not redesign casually)

- Rust 2024 Cargo workspace. MPL-2.0 core; MIT OR Apache-2.0 extension SDK and first-party extensions.
- `winit 0.30.x` owns portable window/input events; Windows rendering uses Direct2D/DirectWrite (hardware or software mode) behind a neutral `RenderBackend`; a `RecordingBackend` exists for headless tests. The document is **never** stored in a toolkit text widget.
- `bareline-document` owns the resident-or-paged `ByteSource` (no mmap in v1), piece tree, snapshots, revisions, the `DocumentService` actor and edit transactions. The valid UTF-8 text view and original byte domain are distinct; undecodable spans retain original bytes through provenance metadata, and large transcodes use disk-backed storage (FC-01/02).
- Canonical document positions are byte offsets into the internal UTF-8 sequence; line/column/grapheme are derived views.
- UI thread never performs unbounded file I/O, regex, syntax indexing or extension RPC. No async runtime in the editor process.
- Third-party extensions never load inside `bareline.exe`; the extension host is an optional signed download.
- Windows-specific code belongs in `platform-windows`; cross-platform core crates must remain OS-neutral. Native Win32 menus/dialogs are reached only through `PlatformServices`.
- Every user-visible command has a stable command ID.
- Persistent formats are versioned and use atomic writes/migrations: TOML for human-edited files, JSON for machine-written state, binary CRC32C journals.
- Performance is designed in: the startup budget, the dependency policy and continuous `xtask perf` measurement apply to every PR; performance numbers are targets, never merge gates. See `../07_DECISION_LOG.md`.

## Scope to implement

- Add/remove caret; caret above/below
- Select next/all occurrence
- Rectangle keyboard/mouse selection
- Rectangle paste/delete/indent/navigation
- Column Editor: insert number sequence (start, step, zero-padding, base 10/16/8/2) or repeated text across the rectangle (ADR-22)
- Line duplicate/move/join/split/sort/remove duplicate/blank
- Hide Lines and Show hidden lines using PR-003 hidden-line view metadata (ADR-22)
- Whitespace trimming and tab/space transforms
- Case transforms
- Paste Special (plain text, no formatting) and drag-and-drop text move within and between views (ADR-22)
- Optional clipboard history: off by default, in-memory only, 20 entries, cleared on exit, never persisted (ADR-23)
- Bookmark toggle/nav/clear and bookmarked-line actions
- Comment command IDs and `CommentProvider` hook only; behavior is PR-013 (ADR-13)
- Command IDs and default shortcuts registered from `crates/editor-surface` via `register_commands` (ADR-15)

## Explicit non-goals

- No comment toggle behavior; register `editor.comment.toggleLine`, `editor.comment.toggleBlock` and the `CommentProvider` hook that reports unavailable until PR-013 supplies language metadata
- No macro recording yet
- No edits to `crates/commands`; that crate is owned by PR-011

## Likely files / ownership

- `crates/editor-surface/**`
- `tests/editing/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Command registration

All commands in this PR are declared in `crates/editor-surface` and registered through `pub fn register_commands(registry: &mut CommandRegistry)`, which `crates/app` calls at startup. Do not add files to `crates/commands`.

### Unified multi-selection operations

All edit commands accept `SelectionSet` and return one `EditTransaction + SelectionSet`. There is no separate “column editor” mutation path. Before mutation:

1. project rectangle selection to line-local ranges using display columns;
2. snap text-mode endpoints to legal UTF-8 and grapheme boundaries;
3. sort ranges by byte offset;
4. merge only when command semantics require it;
5. build edits against one revision;
6. apply in one transaction through `DocumentService`.

### Display columns

Tabs and wide grapheme clusters mean visual/display column differs from character count. Define a reusable `DisplayColumnMap` derived from shaped/layout metrics and tab width. Rectangle typing/paste uses that mapping and pads short lines with configured spaces/tabs deterministically.

### Column Editor

`editor.column.insert` opens a small PR-023 popover with two modes. **Number mode:** initial value, increment, leading zeros toggle, base (decimal, hex, octal, binary), optional repeat count per line. **Text mode:** insert the same text on every line of the rectangle. Both generate one `EditTransaction` over the projected per-line ranges, pad short lines using `DisplayColumnMap`, and produce exactly one undo entry. Works with a zero-width rectangle (caret column on N lines).

### Multi-cursor commands

Required: add caret above/below, Ctrl+Click toggle, select next occurrence, select all occurrences, skip occurrence, undo last added occurrence, rotate primary, duplicate selections, line-wise expand, and escape to primary. Commands must have stable IDs and deterministic selection ordering.

### Line operations

Implement line duplicate, move up/down, join/split, sort ascending/descending (case and numeric options where specified), remove empty/duplicate/consecutive duplicates, trim whitespace, indent/unindent, tabs/spaces conversion and bookmark-line operations. Run transforms through streaming/range operations when selection is large; do not materialize the entire document for “sort selected lines” beyond the selected data budget.

### Hide Lines

`editor.lines.hide` adds the selected logical line range to `EditorViewState.hidden_ranges` (PR-003); `editor.lines.showAll` and a gutter affordance reveal them. Hidden ranges are view metadata only: they never change bytes, never affect save output, and are not persisted in the file. Moving a caret into a hidden range reveals it. Cloned views have independent hidden ranges.

### Paste Special and drag-and-drop

`editor.paste.plainText` pastes the clipboard's plain-text representation only. Drag-and-drop of selected text within a view or between two views is a move by default and a copy with Ctrl, implemented as one transaction per document (delete + insert) so a single undo restores both.

### Clipboard column metadata

When copying a rectangle, place plain text normally and an optional Bareline private clipboard format describing rectangle row widths. Pasting plain text into a rectangle remains deterministic even without private metadata. Never depend on private metadata for data correctness.

### Clipboard history

Behind `editor.clipboard.history_enabled` (default false). When enabled, the last 20 copied text entries are kept in process memory only and shown by `editor.paste.fromHistory` in a PR-023 popover. The list is never written to disk, is cleared on exit, and the feature allocates nothing when disabled.

### Comment hook

Register `editor.comment.toggleLine` and `editor.comment.toggleBlock`. Define `trait CommentProvider { fn tokens_for(&self, document) -> Option<CommentTokens> }` with a default implementation returning `None`; the commands report "no comment definition for this language" until PR-013 installs a real provider.

### Bookmarks

Bookmarks bind to document byte/line anchors and map through edit deltas. Persistent workspace bookmarks are optional metadata; ephemeral bookmarks never modify the file.

## Minimum verification scenarios

- Reproduce Notepad++ issue-class UTF-8 rectangle pastes with multibyte bullets/emoji and prove no byte corruption.
- Rectangle copy/paste across tabs, CJK wide glyphs and short lines.
- 10k cursor insertion/delete/undo produces exactly one undo entry.
- Line sort/remove-duplicate on mixed EOL selection preserves configured EOL policy.
- Column Editor inserts padded sequences across a rectangle with short lines, in all four bases, as one undo entry.
- Hidden lines are view-only and do not change bytes or save output.
- Clipboard history is empty after restart and absent when disabled.
- Comment commands report unavailable through the hook before PR-013 and dispatch to a stub provider in tests.

## Implementation sequence

1. Mark `PR-006` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
2. Add/confirm the public types and interfaces required by this PR before wiring UI details.
3. Implement core behavior with unit/property tests at the owning crate level.
4. Wire command IDs and UI state. UI event handlers must dispatch to commands/services instead of containing document/search business logic.
5. Add failure/cancellation handling for any file/background/process operation.
6. Run the focused tests for changed crates, then the workspace compile/clippy set needed to catch interface breakage.
7. Update the tracker with implementation commit and set **Implemented = DONE**. Verification/testing/acceptance may be performed by separate agents according to project workflow.

## Required test categories

- Happy-path unit tests for every new public behavior.
- At least one error-path test for each filesystem/process/FFI boundary introduced by this PR.
- Regression tests for any bug discovered while implementing.
- Cross-platform compile tests for neutral crates touched by this PR.
- No giant binary fixtures: generate large data during test/benchmark setup.

## Performance and safety constraints

- Do not materialize an entire file before its first viewport is interactive, and never fully materialize a file above the resident threshold in RAM (ADR-01).
- Bound background work and make it cancellable when user action can supersede it.
- Avoid new always-running timers/threads when event-driven behavior is possible.
- Treat file paths, encodings, session data and extension data as untrusted input.
- Never mutate a user file before a complete replacement is ready unless the operation is explicitly an in-place user request and has a safe failure design.
- Only crates from the approved dependency list (ADR-09) may be added; justify each new dependency in the PR description.

## Acceptance checklist

- [ ] UTF-8 rectangle paste cannot corrupt multibyte characters
- [ ] All multi-cursor mutations are one undo transaction
- [ ] Line operations have golden tests for CRLF/LF/mixed EOL
- [ ] Column Editor and Hide Lines have golden tests
- [ ] Clipboard history never persists
- [ ] Comment commands are registered with a hook only; no behavior implemented here
- [ ] Selection/caret semantics documented in code
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-006: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-01, FC-02, FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-005, FR-006, FR-008. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Power editing; budgets; comment provider hook. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-006-01** — Column insertion across tabs, wide glyphs and short lines follows display columns and undoes in one transaction.
- [ ] **AC-006-02** — Enable clipboard history then exceed 20 entries and 16 MiB total; bounded eviction occurs and history never persists to disk.
- [ ] **AC-006-03** — Invoke comment commands before PR-013; show unavailable with a reason and do not mutate text.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
