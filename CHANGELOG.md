# Changelog

All notable changes to Bareline are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/). Preview releases are tagged `v<version>-preview.<date>[.<n>]`. During the preview, versions do not promise [Semantic Versioning](https://semver.org/) compatibility. Entries name the review IDs of the fixes they describe; the matching commit subjects carry the same IDs.

## [Unreleased]

### Added

- More encodings: ISO-8859-2 to ISO-8859-16 (except -9 and -11, which Windows-1254 and Windows-874 cover), KOI8-R, KOI8-U, Windows-874 (Thai), Mac Roman, Mac Cyrillic and the DOS code pages OEM 437, 850, 852 and 866. **Encoding > Interpret As** and **Convert To** list Unicode first, then one submenu per region or script. These encodings are never detected automatically; choose them explicitly. (BIZ-09)
- Syntax highlighting for 51 more languages, for 66 in all. Bareline now bundles every Lexilla 5.5.3 lexer and adds PowerShell, Batch, Shell (Bash), YAML, Markdown, INI, Properties, PHP, Perl, Ruby, Lua, Makefile, Dockerfile, CMake, Diff, Log, Visual Basic, VBScript, Pascal, Fortran, Assembly, LaTeX, R, Swift, Kotlin, Scala, Groovy, Dart, Haskell, Erlang, Tcl, AutoIt, NSIS, Inno Setup, Registry, SCSS, Less, Ada, D, F#, Julia, Lisp, MATLAB, Nim, OCaml, Verilog, VHDL, Zig, CoffeeScript and GDScript. Files are also recognized by names such as `Makefile`, `Dockerfile` and `CMakeLists.txt`, and by `#!` interpreter lines. (BIZ-03)
- JSON, XML and Hex tools are built in, under **Tools > JSON, XML and Hex**: Format, Minify and Validate JSON; Format and Validate XML; XPath Query with the line and column of each match; and Hex View, a read-only tab of a saved file's original bytes. The tools work on the selection or the whole document off the editor thread, format as one undoable edit using the indentation settings, and move the caret to the line and column of the first error. Bareline 1.0 ships without third-party plugins. (BIZ-04)

### Changed

- Regular-expression `^` and `$` now match at every line boundary by default. Start a pattern with `(?-m)` to anchor at the document start and end instead. (SRC-01)
- Regular-expression mode has a **. matches newline** toggle in the find bar and a matching **Search > Options** command. The option is recorded in macros and applies to current-document, open-document and folder searches. (SRC-02)
- About, `--version` and **Copy diagnostics** show the preview version and build commit, with links to the repository and security policy. (BIZ-12)
- Preview executables include the C and C++ runtime, so the Microsoft Visual C++ Redistributable is no longer needed. (BIZ-06)
- Opening a binary-like file shows a notice above the document with **Edit as text** and **Close**, instead of a pop-up menu. The notice has its own band, so it no longer covers the first lines. (UI-01)
- A file open that fails keeps its tab with the error and offers **Retry** and **Open read-only (large-file mode)**. Opening the same path again reuses that tab. (FIO-01)
- The README lists known issues in this preview and a maturity label for each feature area. (BIZ-26, BIZ-27)
- Contributor documentation: issue forms, including a P0 data-loss form, a pull request checklist, code owners, maintainers, support, changelog, the Contributor Covenant 2.1, and an AI-assisted development policy. The bug form asks whether a problem is a regression and which version last worked. (BIZ-20, BIZ-21)
- Bareline draws in software by default, which shows the first frame sooner and uses less memory than hardware drawing. `--hardware` or the **Drawing mode** setting selects Direct2D hardware drawing, which now starts after a first frame drawn in software. Background worker threads start when they are first needed instead of at launch. (PERF-02)

### Fixed

- A failed or unqueued checkpoint of a resident document could clear the only durable checkpoint, leaving nothing to recover. (REC-01)
- Startup no longer deletes recoverable journals. Journals that cannot be read are listed so they can be deleted deliberately, instead of being removed silently. (REC-02)
- Recovery journals from separate instances (`--new-instance`, `--no-session`, `--no-extensions`, or a failed handoff), and journals whose process ID was reused after a restart, are now offered in the Recovery Center. (REC-02, REC-03)
- Reloading from disk or using **Interpret As** on an edited document now discards its old recovery data and history the same way closing does, so recovery no longer fails for the reloaded document. The dirty-reload confirmation now asks to reload rather than to close. (REC-04, REC-15, WSP-11)
- Signing out of or shutting down Windows with unsaved changes now writes the session and recovery checkpoints first, and Windows shows why Bareline is delaying shutdown. (REC-05)
- After an internal error, Bareline waits up to 3 seconds for queued recovery checkpoints to finish before it exits, and records the outcome in the crash log. (REC-06)
- Bareline no longer exits without a message at startup. A damaged, oversized or UTF-16 `settings.toml` is set aside or converted with a notice and the editor starts with defaults, drive-relative paths such as `C:notes.txt` open, and a missing `APPDATA` or `LOCALAPPDATA` no longer disables recovery. A launch that hands its files to a running instance leaves the settings file alone. (APP-01, APP-02, APP-16, APP-17)
- Replace All on large documents stays one undo step and reports refusals in readable text instead of stopping at "Preparing replacement...". Large-file (paged) Replace All refuses more than 10,000 matches before changing anything. (EDT-02, PED-17)
- In large-file (paged) mode, rectangular selections, Shift+navigation anchors, auto-pairing, overtype, and line commands at UTF-8 boundaries no longer corrupt text or apply to the wrong lines. (PED-01, PED-02, PED-03, PED-05)
- Regular expressions handle CRLF line endings: `.` no longer matches the CR, `$` matches before CRLF, and Replace All no longer rewrites CRLF files to LF. (SRC-01)
- The 4 MiB limit on Windows clipboard copy and paste is lifted. The limit is now the `clipboard.max_bytes` setting (1 GiB by default), with a warning above 256 MiB. Pasting or cutting tens of megabytes works, an empty or non-text clipboard is ignored, a busy clipboard is retried, and search and other single-line fields accept at most 16 KiB. (EDT-07, UI-14)
- Ordinary UTF-8 files with accented or CJK text open in the normal editor instead of failing, and a file that exceeds the in-memory budget falls back to large-file mode, including on reload. (FIO-01)
- Comparing large (paged) files aligns them on matching lines, so one inserted line no longer marks the rest of the file as changed. Large insertions and removals are reported where they are, and with **Ignore blank lines** a run of blank lines is no longer reported as a change. (SRC-05)
- Syntax colors no longer flash to plain text after an edit; the previous colors stay until the new ones arrive. An edit, or a scroll back up, re-highlights from just before the visible text instead of from the start of the file, with either lexer. (SRC-14)

### Security

- Windows executables are built with Control Flow Guard, and preview packaging rejects executables that lack Control Flow Guard, do not record the expected build commit, or import the Visual C++ runtime DLLs. CET shadow-stack compatibility is not declared, because the regular-expression and extension-host JIT compilers do not support it. (SEC-15)

## [0.1.0-preview.20260928.1] - 2026-09-28

### Changed

- Preview packages are built and verified by an automated release workflow. Signing preparation is documented in the code signing policy; the packages remain unsigned.

## [0.1.0-preview.20260928] - 2026-09-28

### Added

- First public Windows x64 preview, as a per-user installer and a portable ZIP.

[Unreleased]: https://github.com/TheWoovee/BareLine/compare/v0.1.0-preview.20260928.1...HEAD
[0.1.0-preview.20260928.1]: https://github.com/TheWoovee/BareLine/compare/v0.1.0-preview.20260928...v0.1.0-preview.20260928.1
[0.1.0-preview.20260928]: https://github.com/TheWoovee/BareLine/releases/tag/v0.1.0-preview.20260928
