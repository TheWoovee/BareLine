# Research Baseline — Notepad++ Parity and Problems to Solve

**Research date:** 2026-09-05  
**Notepad++ baseline used for parity:** 8.9.8 (build dated August 23, 2026)

This file records why Bareline exists. It is evidence, not a requirement substitute. Product requirements are in `01_REQUIREMENTS.md`.

## What Notepad++ does exceptionally well

Notepad++ remains a strong benchmark because it is fast to install, launches quickly, works offline, has a long-lived plugin ecosystem, supports many languages, gives users powerful regex/search/editing capabilities, and exposes almost every action through menus or shortcuts. Bareline must preserve those strengths rather than “modernizing” them away.

## Verified feature families that Bareline must cover

The current Notepad++ manual organizes the product around file handling; editing; searching/replacing; views; sessions/workspaces/projects; function list; auto-completion; built-in and user-defined syntax highlighting; macros; external commands; plugins; command-line operation; preferences/configuration; themes; localization; shell integration; upgrading; and the full menu/tab/status/tray surface. Bareline maps every productivity feature family to a native requirement.

## Problems and disadvantages to address

### 1. Huge-file behavior is not seamless

Notepad++'s own documentation exposes a “Large File Restriction” that can disable syntax highlighting and other features for performance. Community maintainers have explained that the editor component keeps the file in a single buffer and is not fundamentally designed as a paged huge-file editor. Reported symptoms include slow folding, slow search-result navigation, unresponsiveness, and inability to comfortably work with hundreds of megabytes or multi-gigabyte text.

**Bareline response:** the document model is not a whole-file mutable string. It uses an immutable `ByteSource` (streamed into memory for files up to 256 MiB, paged on demand above that; no memory mapping in v1, see ADR-01) plus a piece tree/edit store and sparse/lazy indexes. The text view is valid UTF-8 with original-byte provenance (ADR-02, FC-01). Opening publishes the first viewport before the rest of the file is read, and scrolling never requires lexing the entire file. Search, syntax and outlines are cancellable background work that prioritize the viewport.

### 2. Session/backup semantics can surprise users

The Notepad++ community repeatedly warns that periodic backup/session snapshots are not a substitute for actually saving files. There are reports involving session save errors in synchronized locations and an open 2026 issue where an update lost pinned tabs.

**Bareline response:** distinguish clearly between **saved file**, **crash-recovery journal** and **workspace/session state**. Use atomic writes, checksummed journals, recovery UI, retention policies and deterministic migration tests. Never use the recovery cache as an ambiguous pseudo-file store. A per-file **local history** is a planned v1.1 addition (ADR-24), kept separate from recovery so neither is mistaken for a save.

### 3. Plugin compatibility is coupled to host internals

The Notepad++ FAQ explains that plugins can stop working when plugin communication rules, directory structure or Scintilla interfaces change. In-process native DLL plugins also enlarge the blast radius of a faulty extension.

**Bareline response:** no third-party code loads into the editor process. Extensions run in a separate host process with a versioned RPC/capability contract. Extension crashes cannot corrupt the editor process. Cross-platform extension packages use WebAssembly components where practical.

### 4. Recent releases show meaningful path/update/security attack surface

The 8.9.7/8.9.8 change history includes fixes around session path normalization, updater path traversal/TOCTOU, authenticode verification, backup path traversal and UNC/NTLM exposure, among others.

**Bareline response:** Rust for the core, canonical path/trust checks, no automatic untrusted UNC access, signed update manifests plus package signatures, atomic update staging, least-privilege extension host, safe archive extraction and explicit security logging.

### 5. Multi-editing and column editing have accumulated inconsistent edge cases

Notepad++ historically distinguishes column mode from multi-selection behavior, and a September 2026 bug report demonstrates a UTF-8 column-paste corruption edge case in 8.9.8.

**Bareline response:** one selection model represents stream, rectangular and N-cursor selections. Editing commands operate over normalized selection ranges with Unicode grapheme/byte boundaries tested explicitly. Column mode is a projection of the same multi-cursor engine, not a separate editor behavior.

### 6. UI discoverability can become menu/config heavy

Notepad++ exposes a very large set of menus, toolbar buttons, preference pages and XML-based configuration. Power is excellent; discoverability and visual consistency can be improved without removing menus.

**Bareline response:** keep familiar top menus and direct shortcuts, but add one searchable command palette, one unified settings surface, one unified search panel, progressive disclosure, consistent theming and sensible defaults. No ribbon and no IDE dashboard.

### 7. Windows-only core architecture limits portability

Notepad++ is explicitly a Windows editor.

**Bareline response:** platform-specific behavior lives behind `PlatformServices`. The editor/document/search/syntax/session/extension protocols are platform-neutral Rust crates. Windows gets first-class shell integration now; Linux and macOS adapters can be added without rewriting the editor engine.

### 8. A cross-platform GUI toolkit is not automatically the lightest route

Bareline's Windows v1 is intentionally rendered with Windows' built-in Direct2D/DirectWrite stack through a narrow renderer contract, while winit supplies portable window/input events. This avoids coupling the virtualized editor to a general text widget and avoids bundling a browser or large graphics framework. Both hardware and software Direct2D modes are implemented and the lighter one becomes the default from measurement (ADR-32). The menu bar, context menus and common dialogs stay native Win32 so they cost nothing to build and are accessible by default (ADR-04). The tradeoff is a small custom control layer, owned by PR-023 and separated from all document/search logic.

### 9. Notepad++ ships fast because it does almost nothing at startup

Notepad++ cold-starts in a few hundred milliseconds and idles around 10 to 20 MB of private memory because it reads a handful of XML files, creates one Win32 window and defers everything else. Any replacement that adds an async runtime, a font enumeration pass, an icon rasterizer or a session restore before the first frame will lose that comparison regardless of architecture.

**Bareline response:** a written startup budget (ADR-33), an approved-dependency list that bans async runtimes and heavyweight crates from the editor process (ADR-09), and continuous measurement from the first PR (ADR-10). Performance is treated as a design constraint on every PR, not a gate at the end.

## Source verification

Sources span multiple dates and were reviewed on 2026-09-05. Issue #16922 dates to 2025-08-15; it is not a 2026 report. Re-verify each link before publishing this baseline in the open-source repository; a dead or misquoted reference should be replaced, not paraphrased.

## Product decisions explicitly *not* copied from Notepad++

- No Scintilla editing buffer.
- No in-process native plugin ABI.
- No XML as the primary user configuration format.
- No “large file mode” that simply switches useful features off globally.
- No implicit access to remote/UNC paths without trust handling.
- No mandatory administrator elevation for normal configuration/update tasks.
- No separate behavioral model for rectangle selections versus multiple cursors.

## Sources

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

## v1.2 evidence and claim discipline

Issue reports identify reproducible scenarios to investigate; they do not establish current failure on every machine or a measured advantage for Bareline. Record issue creation date, affected version, current resolution and actual reproduction independently. A page unavailable to the research tool is not necessarily a dead link. The initial review verified issue context and library limitations; refresh release/security claims before external publication.

Pin the Notepad++ 8.9.8 executable hash for baseline trials and record a newer stable version separately. No comparative benchmark has run in this package. Architecture/performance explanations above are design hypotheses until FC-10 measurements exist. [Original review](REVIEW_2026-09-05.md) preserves detailed source checks; [foundation contracts](09_FOUNDATION_CONTRACTS.md) record codec, PCRE2 and trust references.
