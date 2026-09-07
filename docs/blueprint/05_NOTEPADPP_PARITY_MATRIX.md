# Notepad++ Core Feature Parity Matrix

**Version:** 1.2 (2026-09-05)

This matrix is a release checklist, not a claim that Bareline should copy Notepad++ UI or implementation details. Every row must end in one of these states before Windows v1 is accepted (PR-021):

| Delivery route | Meaning |
|---|---|
| `Planned` | scheduled in a PR, not yet implemented |
| `Implemented` | same capability, same or better UX |
| `Equivalent` | planned alternative UX; equivalence requires evidence |
| `Excluded` | deliberately not built; reason recorded |
| `v1.1` | decided and deferred to the next release |
| `Extension` | planned first-party extension route; not verified delivery |

## Feature families

| Notepad++ capability family | Bareline requirement / owner | Improvement in Bareline | Delivery route |
|---|---|---|---|
| New/Open/Save/Save As/Save All/Save Copy | FR-001/002, PR-004 | atomic replace + recovery separation; viewport-first open | Planned |
| Reload from disk, Set read-only | FR-001/003, PR-004 | explicit commands, editor lock separate from file attribute | Planned |
| Recent files / tabs / pin / sort / document switcher | FR-003, PR-004 (model), PR-010 (UI) | cleaner tab model; update-safe persistence | Planned |
| Restore last closed tab | FR-001, PR-004 | caret and pin state restored | Planned |
| Dual view / clone / synchronized scroll | FR-004, PR-010 | same document model; independent views | Planned |
| Move to new instance | FR-004, PR-018 | opens in a new process via IPC handoff | Equivalent |
| Undo/redo/cut/copy/paste/line editing | FR-005, PR-003/006 | unified transactions | Planned |
| Paste Special, drag-and-drop text | FR-005, PR-006 | | Planned |
| Hide Lines | FR-005, PR-006 | view-only, never changes bytes | Planned |
| Clipboard History panel | FR-005/029, PR-006 | opt-in, in-memory only, off by default | Planned |
| Column mode / multi-editing | FR-006, PR-003/006 | one selection engine, UTF-8-safe | Planned |
| Column Editor (number sequences) | FR-006, PR-006 | bases 10/16/8/2, padding, step | Planned |
| Find/replace/count/mark/find in files | FR-007, PR-005 | streaming/cancellable, huge-file safe | Planned |
| Replace in files / projects | FR-007, PR-026 | preview-first, per-file atomic, encoding-preserving | Planned |
| Mark with style tokens 1–5 | FR-007, PR-005 | five per-view styles with stable IDs | Planned |
| Go to / bookmarks / brace match | FR-008, PR-006 (bookmarks, go to), PR-013 (brace match) | byte-offset navigation + persistent option | Planned |
| Encoding/character sets/EOL conversion | FR-009, PR-007 | valid text view, original-byte provenance, streaming save | Planned |
| Syntax highlighting / folding | FR-010, PR-008 | Lexilla without Scintilla buffer; viewport-first; native fallback | Planned |
| Fold All / Unfold All / Fold Level N | FR-010, PR-008 | acts on known folds, continues as indexing completes | Planned |
| User Defined Language | FR-011, PR-008 | TOML schema + N++ UDL importer; UDL engine doubles as lexer | Planned |
| Auto-completion/function hints | FR-012, PR-013 | bounded local indexes | Planned |
| Word wrap/symbols/guides/zoom/fullscreen/always-on-top | FR-013, PR-003/012 | consistent modern theming | Planned |
| Document map | FR-014, PR-009 | progressive for multi-GB files | Planned |
| Function List | FR-015, PR-009 | progressive/cancellable; imports N++ functionList definitions | Planned |
| Folder as Workspace / projects | FR-016, PR-009 | no forced project metadata | Planned |
| Sessions / workspace restore / backups | FR-017, PR-004 | checksummed recovery journal + clear recovery center | Planned |
| Monitoring (tail -f) | FR-018, PR-015 | rotation/truncation handling on paged source | Planned |
| Macros | FR-019, PR-014 | stable command IDs and TOML format | Planned |
| Ghost/replay typing | FR-019, PR-014 | macro action rather than special subsystem | Planned |
| Run external commands | FR-020, PR-014 | safe direct spawn + captured output | Planned |
| Hash tools | FR-021, PR-017 | same parity, clearer security labels | Planned |
| Print / export | FR-021, PR-017 | first-party utility path | Planned |
| Compare (N++ plugin) | FR-021A, PR-025 (engine), PR-017 (UI) | built-in, five semantic states, configurable per theme | Planned |
| Compare inline view, folder compare | FR-021A | | v1.1 |
| Plugins / Plugins Admin | FR-022, PR-016 | isolated versioned extension host, optional download | Planned |
| Seed XML/JSON/Hex capabilities (not ecosystem breadth) | FR-022, PR-027 | first-party JSON tools, XML tools, Hex view; SDK reference | Extension |
| Spell check, CSV tools, LSP client | FR-022 | community extensions | v1.1 |
| Preferences / Style Configurator / Shortcut Mapper | FR-023, PR-011/012 | searchable settings + TOML, keys visible | Planned |
| Localization (N++ ships ~90 languages) | FR-024, PR-012 | English-only v1; community packs planned; language breadth deferred | v1.1 |
| Command line args / multi-instance | FR-025, PR-018 | versioned IPC | Planned |
| Explorer “Edit with…” / file associations | FR-026, PR-018 | opt-in and portable-safe | Planned |
| Replace Windows Notepad | FR-026 | not offered; user sets default app manually | Excluded |
| System tray | FR-026, PR-018 | same behaviors with explicit settings | Planned |
| Update / debug info | FR-027, PR-018/020 | signed staged update + rollback | Planned |
| Portable package | FR-028, PR-018 | fully relocatable persistent data | Planned |
| Character Panel | | | v1.1 |
| Post-It mode (frameless) | FR-013 | fullscreen/distraction-free covers the use case | v1.1 |
| Document Peeker (tab hover preview) | | | v1.1 |
| Local history (not in N++) | FR-017 | planned differentiator | v1.1 |
| Integrated terminal (not in N++) | FR 2.2 | out of scope by design | Excluded |
| Source control panel (not in N++) | FR 2.2 | out of scope by design | Excluded |
| Telemetry (not in N++) | FR 2.2 | none, ever | Excluded |

## Rules

- PR-021 may not accept a row in `Planned`.
- `Excluded` and `v1.1` rows carry a reason here or in `07_DECISION_LOG.md`.
- Rows are added, never silently removed. A capability discovered late in the Notepad++ manual gets a row before it gets code.

## v1.2 evidence status

Every feature row is **NOT_STARTED** for implementation and verification. Equivalent and Extension describe intended delivery routes, not achieved parity. English-only v1 does not match localization breadth, and three seed extensions do not match an ecosystem. [Acceptance register](10_ACCEPTANCE_AND_TRACEABILITY.md) maps requirements to owners and review cases. PR-021 must add the exhaustive command inventory and attach results before setting Implemented or Verified.
