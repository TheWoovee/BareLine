# Bareline

Bareline is a native Windows text and code editor written in Rust, with tabbed documents, split views, large-file editing, search and replacement, syntax highlighting, comparison, macros, and recovery.

**Status: Windows x64 preview.** The application is usable for evaluation, but compatibility, accessibility, recovery, and release qualification are still in progress. Preview packages are unsigned. Automatic updates, extension downloads, and extension execution are disabled in the default build.

[Download the Windows preview](https://github.com/TheWoovee/BareLine/releases) · [Report a bug](https://github.com/TheWoovee/BareLine/issues) · [Contribute](CONTRIBUTING.md)

[Code signing policy](CODE_SIGNING.md) · [Privacy policy](PRIVACY.md)

## How Bareline differs

These are design facts of the current preview, not performance claims. Comparative speed and memory figures are published only from a qualified release benchmark.

- **Large files are paged, not loaded whole.** Files larger than 256 MiB (the **Large-file threshold** setting) open in large-file mode: Bareline reads 1 MiB pages as you move through the file and caches at most 64 MiB of each such file's pages in memory by default. You can scroll, search, edit and save them; line numbers are estimated until a background count finishes.
- **Unsaved work is journaled.** Edits to open documents, including Untitled ones, are written to recovery journals in your profile in the background. After a crash or a forced shutdown, the **Recovery Center** offers each document to restore, compare with the file on disk, or save elsewhere. When Windows signs out or shuts down, Bareline writes its session and recovery checkpoints first; after an internal error it waits up to 3 seconds for queued checkpoints before exiting.
- **Compare is built in.** Compare two documents, a document with a file on disk, or a document with its last saved version, and copy changes between the sides, without a plugin.
- **Replace in Files can be reviewed and rolled back.** Folder-wide replacement shows a preview where you choose the changes, keeps backups and a receipt for each job, can restore the backups later, and reconciles a job that was interrupted by a crash at the next start.
- **JSON, XML and Hex tools are built in.** Formatting, validation, XPath and a read-only Hex View are part of the editor. Bareline 1.0 loads no third-party plugins.
- **No telemetry.** Bareline has no account, analytics, telemetry or crash-report upload, and preview builds make no update requests. See the [privacy policy](PRIVACY.md).
- **Mostly memory-safe code.** The editor is written in Rust. Its C and C++ code is limited to the bundled Lexilla lexers and the PCRE2 regular-expression engine with its JIT compiler, and it loads no plugins.

What it does not have yet: plugins, user-interface languages other than English, and builds for platforms other than Windows x64. Screenshots will be added with a future preview release.

## Install and run

Bareline targets **64-bit Windows 10 22H2 (build 19045) or later, including Windows 11**. The installer enforces this: it refuses to install on Windows versions older than 10.0.19045 and on computers that cannot run x64 programs (`MinVersion=10.0.19045`, `ArchitecturesAllowed=x64compatible`). Because Windows 11 on Arm runs x64 programs, the installer also accepts it; that combination is untested. The portable ZIP performs no version check, and older Windows builds are untested and unsupported. Final clean-machine qualification of the supported versions remains pending. Native Linux, macOS, ARM64, and 32-bit releases are not available.

The executables include the C and C++ runtime, so no Microsoft Visual C++ Redistributable is needed; packaging rejects a build that imports it. The first two previews (`v0.1.0-preview.20260928` and `v0.1.0-preview.20260928.1`) still required the **Microsoft Visual C++ v14 Redistributable (x64)**; if one of those reports a missing `MSVCP140.dll`, `VCRUNTIME140.dll`, or `VCRUNTIME140_1.dll`, install that redistributable from [Microsoft's official download page](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) or use a newer preview.

Open the [Releases page](https://github.com/TheWoovee/BareLine/releases) and read the notes for the selected preview. Release assets and their checksums are listed there. GitHub's automatically generated **Source code** archives contain source, not a ready-to-run application.

### Portable ZIP

1. Download the Windows x64 portable ZIP from a release.
2. Extract the entire archive into a writable folder.
3. Run `bareline.exe`.

Keep the empty `bareline.portable` file next to the executable. It makes Bareline store settings, sessions, recovery, and other profile data in the adjacent `data` folder. Move that folder with the application if you want to keep your profile. Do not run directly from inside the ZIP. If the `data` folder cannot be written, for example on write-protected media, Bareline shows a warning, does not save the session, and keeps recovery journals in `%LOCALAPPDATA%\Bareline\portable-recovery` until the folder is writable again.

The repository also contains a [Scoop manifest](packaging/scoop/bareline.json) for the portable ZIP; it is not yet published in a Scoop bucket. A copy installed through it stores its profile in `%LOCALAPPDATA%\Bareline`, because the manifest removes the portable marker.

### Windows setup

If the selected release includes a setup executable, run it and follow the installer. The installer defaults to a per-user installation; optional Explorer integration does not make Bareline your default editor. Installed launches use `%LOCALAPPDATA%\Bareline` for profile data.

These preview executables are not Authenticode-signed, so Windows may display an unknown-publisher or reputation warning. Check the release source and published SHA-256 checksum before deciding whether to run them. For example:

```powershell
Get-FileHash .\bareline-0.1.0-windows-x64-portable.zip -Algorithm SHA256
```

Preview updates are manual: download a newer release and follow its release notes. The in-app updater is unavailable in this build.

### Uninstall

Close Bareline before removing it. For an installed copy, use **Settings → Apps → Installed apps** on Windows 11 or **Apps & features** on Windows 10, select Bareline, and choose **Uninstall**. The installer removes its application files and registered optional Explorer integration, while retaining your profile, recovery data, and documents.

For a portable copy, delete the extracted application folder after copying any documents or profile data you want to keep. Its `data` folder can contain recoverable unsaved work. Removing an installed copy does not delete `%LOCALAPPDATA%\Bareline`; remove that folder separately only after preserving anything you need. Older `%APPDATA%\Bareline` profiles, if present, are retained too.

## Features

Open the **Command Palette** with **Ctrl+Shift+P** to find commands by name. Menus and the palette reflect the current document and operation; some commands become available only after selecting text, opening a folder, or starting the relevant tool.

| Area | Maturity | Available in the editor |
| --- | --- | --- |
| Files and tabs | Preview | New/open/save, Save As, Save Copy, Save All, read-only documents, restore the last closed tab, Close All/Others/Left/Right, copy the path, name or folder, rename, open or move a saved document to a new window, files dropped on the window, tab reordering, pinning, colors, sorting, vertical tabs, an all-tabs list, and a recent-document switcher. |
| Editing | Preview | Undo/redo, multiple carets, select next/all occurrences, rectangular editing, a column editor and Column Selection Mode, Insert/Overwrite, plain-text paste, and optional in-memory clipboard history. |
| Text operations | Stable | Duplicate/move/join/split lines, case conversion, indentation, tabs/spaces conversion, whitespace trimming, line sorting including numeric sort, duplicate removal, and empty/blank-line removal. |
| Navigation | Stable | Go to line, go to or select to the matching brace, bookmarks and bookmarked-line operations, hide/show selected lines, a document list, document map, and language-aware outline. |
| Split views | Stable | Horizontal or vertical splits, clone or move a document to the other view, separate view focus, and synchronized horizontal/vertical scrolling. |
| View | Experimental | Zoom in/out/reset, full screen, always on top, word wrap, line numbers, whitespace and end-of-line symbols, indent guides, an edge line, brace highlighting, and a horizontal scroll bar. |
| Search | Preview | Literal, extended, and PCRE2 regular-expression modes with line-based `^`/`$`; case, whole-word and ". matches newline" options; search in a selection, current document, open documents, or folder; next/previous results; and five mark styles. |
| Replacement | Preview | Current-document replacement plus file/workspace replacement with a reviewable preview, selected changes, preserve-case option, backups, cancellation, rollback, backup management, and recovery of interrupted jobs at startup. |
| Languages | Preview | 66 built-in languages with syntax highlighting, language selection and file associations, comments, folding, completion, static signatures, and import/export of user-defined language definitions. |
| Workspace | Stable | Folder tree, refresh and incremental loading, file/folder creation, rename, deletion to the Recycle Bin, and document/outline panels. |
| Comparison | Preview | Side-by-side document comparison, compare with a disk file, the current disk version or the last saved version, next/previous difference, copy a difference or selected range between sides, whitespace/case/EOL/BOM options, and configurable difference colors. |
| Encodings | Preview | Unicode and legacy encodings, scored detection with an "encoding may be wrong" hint, reinterpret original bytes, convert the save encoding, control BOM output, and convert LF/CRLF/CR endings for a document or selection. |
| Large files | Preview | Paged editing of files above the large-file threshold, background line counting, search, replacement, comparison, following appended content, and saving. See [Large files](#large-files). |
| Recovery and sessions | Preview | Background recovery journals for open documents, a Recovery Center, recovery on sign-out and shutdown, session restoration, named sessions, pinned recent files and folders, and saving recovered documents to another file. Keep independent backups while acceptance testing is pending. |
| External changes and logs | Preview | External-change checks with keep/reload choices, optional automatic reload for clean local files, and read-only following of appended content with pause/resume controls. |
| Macros and external tools | Preview | Record, name, save, import/export, and replay macros, undone as one step; repeat a macro a fixed number of times or to end of file; assign shortcuts; Run (F5) with Notepad++ variables; run external commands you confirm, with captured output. |
| Utilities | Stable | Document statistics, MD5/SHA-1/SHA-256/SHA-512 hashes, Base64 and URL encode/decode, syntax-colored HTML/RTF export, and printing with font, margin, line-number, header/footer, and color options. |
| JSON, XML and Hex | Experimental | Built in under **Tools > JSON, XML and Hex**: format, minify and validate JSON; format and validate XML and run XPath queries; view a file's original bytes in a read-only Hex View tab. Tools work on the selection or the whole document, format as one undoable edit, and move the caret to the first error. |
| Spell check | Experimental | Underlines misspelled words in plain text and Markdown using the Windows spelling dictionary for your language, with suggestions, **Add to Dictionary** and **Ignore All**. Code is checked only when its language turns it on. |
| Keyboard | Experimental | A Bareline and a Notepad++ shortcut preset, the Shortcut Mapper, and keymap import/export. |
| Customization | Preview | Light/dark/system themes, theme color overrides, fonts, wrapping, indentation, a configurable toolbar, and user/workspace settings. Some settings have no effect yet; see [Known issues](#known-issues-in-this-preview). |
| Accessibility | Preview | UI Automation names, roles and text for the editor, tabs, panels, banners and Settings, with a non-color cue for selected rows. Testing with physical screen readers, high contrast and input method editors is pending. |
| Rendering | Preview | Software rendering by default, which starts fastest and uses the least memory; Direct2D hardware rendering with `--hardware` or the **Drawing mode** setting. |

Maturity: **Stable** areas are complete for this preview, covered by automated tests, and have no open known issue. **Preview** areas work and are covered by automated tests, but have limitations listed under [Known issues in this preview](#known-issues-in-this-preview) or changed significantly in this release cycle. **Experimental** areas are new in this release cycle and may change. No area has completed acceptance testing on clean machines yet.

MD5 and SHA-1 are provided for legacy integrity workflows; use an appropriate modern algorithm for security-sensitive work.

### Languages and encodings

The built-in language catalog has 66 languages, highlighted by the bundled Lexilla lexers: **C, C++, C#, Java, JavaScript, TypeScript, Python, Rust, Go, HTML, CSS, SCSS, Less, JSON, XML, SQL, TOML, PowerShell, Batch, Shell (Bash), YAML, Markdown, INI, Properties, PHP, Perl, Ruby, Lua, Makefile, Dockerfile, CMake, Diff, Log, Visual Basic, VBScript, Pascal, Fortran (free and fixed form), Assembly, LaTeX, R, Swift, Kotlin, Scala, Groovy, Dart, Haskell, Erlang, Tcl, AutoIt, NSIS, Inno Setup, Registry, Ada, D, F#, Julia, Lisp, MATLAB, Nim, OCaml, Verilog, VHDL, Zig, CoffeeScript, and GDScript**. Files are recognized by extension, by names such as `Makefile`, `Dockerfile` and `CMakeLists.txt`, and by a `#!` interpreter line. Language features depend on the selected language and available definitions. Completion and signatures are local editor features; this preview does not provide a language-server or debugger integration.

Supported text encodings are UTF-8, UTF-16 LE/BE, UTF-32 LE/BE, ISO-8859-1 through ISO-8859-16 (except -9, -11 and -12), Windows-874 and Windows-1250 through Windows-1258, KOI8-R, KOI8-U, Mac Roman, Mac Cyrillic, OEM 437, 850, 852 and 866, Shift-JIS, GBK, Big5, EUC-JP, and EUC-KR. Stateful encodings are unsupported. Detection reads up to 64 KiB: it recognizes byte order marks, UTF-8, UTF-16 without a byte order mark, and GBK, Big5, Shift-JIS, EUC-JP and EUC-KR when the sample holds enough characters to tell them apart. Ambiguous legacy input falls back to Windows-1252 with an "encoding may be wrong" hint that names the likely candidates; choose the encoding explicitly with **Encoding > Interpret As** when needed. See the [codec catalog](crates/file-io/src/codecs/CATALOG.md) for aliases and byte-preservation behavior.

### Large files

Files up to **256 MiB** use the resident editor by default; larger files use paged editing. Defaults are 1 MiB pages, a 64 MiB cache per paged document, a 256 MiB aggregate document cache, and a separate 128 MiB undo RAM budget. Undo retains at most 100,000 changes per document. These limits are configurable in Settings.

These are resource defaults, not a guaranteed maximum file size or performance claim. File size, available memory, free disk space, encoding, and the operation affect what can complete. Encoding conversion can use temporary disk storage, with a default quota of 20 GiB. Search and comparison have bounded work and memory limits; unsupported streaming patterns or exhausted limits may prevent a complete result.

### Extensions and updates

**No third-party plugins in 1.0; JSON, XML and Hex tools are built in.** JSON and XML text of up to 16 MiB can be formatted, minified, validated or queried (a large-file mode selection is formatted up to 1 MiB), and Hex View shows the first 1 MiB of a saved file's original bytes. XML tools never load DTDs or external entities. XML formatting keeps documents with mixed content or significant whitespace as they are.

The repository also includes a separate extension host and first-party JSON, XML, and hex-view components. **They are not enabled in the default preview**, even if an extension preference is enabled in Settings. Their download and execution paths require a configured release with reviewed public trust keys, publisher identity, signed metadata, and a separately supplied runtime. This also applies to the in-app update pipeline. Do not treat checked-in test trust fixtures as production configuration.

## Everyday shortcuts

These are the defaults; use **Shortcut Mapper** from the Command Palette to inspect or change bindings. Coming from Notepad++? Set **Settings > Keyboard > Shortcut preset** to Notepad++ to use its shortcuts.

| Action | Shortcut |
| --- | --- |
| New / open / save | Ctrl+N / Ctrl+O / Ctrl+S |
| Save As / close tab / restore closed tab | Ctrl+Shift+S / Ctrl+W / Ctrl+Shift+T |
| Undo / redo | Ctrl+Z / Ctrl+Y |
| Find / replace / find in open documents | Ctrl+F / Ctrl+H / Ctrl+Shift+F |
| Find next / previous | F3 / Shift+F3 |
| Go to line | Ctrl+G |
| Command Palette | Ctrl+Shift+P |
| Previous / next tab | Ctrl+PageUp / Ctrl+PageDown |
| Recent-document switcher / other split view | Ctrl+Tab / F6 |
| Move tab left / right | Ctrl+Shift+PageUp / Ctrl+Shift+PageDown |
| Add caret above / below | Ctrl+Alt+Up / Ctrl+Alt+Down |
| Select next / all occurrences | Ctrl+D / Ctrl+Shift+L |
| Column editor | Alt+C |
| Duplicate / join lines | Ctrl+Shift+D / Ctrl+J |
| Move lines up / down | Alt+Up / Alt+Down |
| Toggle line / block comment | Ctrl+/ / Ctrl+Shift+/ |
| Toggle / next / previous bookmark | Ctrl+F2 / F2 / Shift+F2 |
| Completion / plain-text paste | Ctrl+Space / Ctrl+Shift+V |
| Print / Run | Ctrl+P / F5 |
| Zoom in / out / restore default zoom | Ctrl+= or Ctrl+Numpad Plus / Ctrl+- or Ctrl+Numpad Minus / Ctrl+0 |
| Go to / select to matching brace | Ctrl+B / Ctrl+Shift+B |
| Full screen | F11 |

**Edit > Column > Column Selection Mode** makes a plain drag on the text, or Shift+arrow keys, select a rectangle until you turn it off. Clicks on tabs, the gutter, the status bar and panels work as usual; while the mode is on, Shift+click and double-click on text start a rectangle instead of extending the selection or selecting a word.

**View > Show > Show Whitespace** marks spaces and tabs throughout the editor. When it is off, whitespace is still marked inside a selection, which is the default of the **Show whitespace** setting; set that setting to `none` to hide it everywhere.

## Command line

```powershell
.\bareline.exe "C:\notes\todo.txt"
.\bareline.exe --line 42 --column 5 "C:\src\main.rs"
.\bareline.exe --monitor "C:\logs\app.log"
.\bareline.exe --hardware --no-session --new-instance
```

| Option | Purpose |
| --- | --- |
| `--line N`, `--column N` | Open at a one-based location. `--column` requires `--line`. |
| `--read-only` | Open documents read-only. |
| `--monitor` | Open read-only and follow new content. |
| `--no-session` | Do not restore the previous session, and do not save it on exit. |
| `--no-extensions` | Disable extensions for the launch; default preview builds already disable extension execution. |
| `--new-instance` | Open a separate window instead of handing the files to a running one. |
| `--software`, `--hardware` | Select the renderer for this launch; choose one. Software is the default. Hardware (GPU) drawing starts after the first frame, which is drawn in software. Either flag overrides the **Drawing mode** setting (`renderer.mode`). |
| `-` | Read standard input into a new Untitled document. Bareline waits at most 10 seconds for the input to end and then opens what it has, with a notice. |
| `--help` / `-h`, `--version` / `-V` | Show command-line help or the version. |
| `--` | Treat following arguments as file paths, including names beginning with a hyphen. |

The Notepad++ spellings `-n<line>`, `-c<column>`, `-ro`, `-multiInst`, `-nosession` and `-noPlugin` are accepted, ignoring case; `-c<column>` alone applies to line 1, and `-notabbar` is accepted and ignored. Other Notepad++ switches are refused as unknown options.

`--no-session`, `--no-extensions` and `--new-instance` do not turn off crash recovery. A separate instance keeps its recovery journals in the same profile recovery folder, and the next launch offers them in the Recovery Center.

Files named on the command line open on top of the restored session, in the background. A launch opens at most 16 files; further paths, and paths that cannot be used, are listed in a notice as not opened. A path that does not exist, in a folder that does, opens as a new document that is created when you first save it. Launching without paths opens an Untitled document and may restore the saved session. Internal diagnostic modes are intended for development and are separate from ordinary document launches.

## Settings and local data

Open **Settings** from the Command Palette. The editor exposes common preferences in its settings UI and can open the underlying TOML file for more detailed editing.

| Launch type | Profile location |
| --- | --- |
| Installed or ordinary source-built executable | `%LOCALAPPDATA%\Bareline` |
| Portable executable with adjacent empty `bareline.portable` marker | `<executable folder>\data` |

The profile contains `settings.toml`, `keymap.toml`, `session.json`, and folders for recovery, macros, extensions, and diagnostics as those features are used. Older profiles under `%APPDATA%\Bareline` are handled by the profile migration path; `%APPDATA%` is also the fallback if `LOCALAPPDATA` is unavailable.

Workspace preferences live in `.bareline\settings.toml` under the workspace folder and are ignored until you opt in. The clipboard history setting is off by default and, when enabled, retains a bounded history only in memory for that session. Recovery journals and session files are local application data; preserve the profile when moving or replacing a portable installation. An installed copy also adds opened files to Windows Recent items unless you turn off **Add opened files to Windows Recent items**; the [privacy policy](PRIVACY.md) lists all locally kept data and network use.

## Build from source

You need:

- Windows x64 and the MSVC Rust target (`x86_64-pc-windows-msvc`).
- [Rust installed through rustup](https://rust-lang.org/tools/install/). The repository pins **Rust 1.98.1** in [rust-toolchain.toml](rust-toolchain.toml); older compilers are not supported by this workspace.
- [Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload, an MSVC compiler/linker, and the Windows SDK. The syntax bridge builds bundled C++17 sources.
- Internet access for the first dependency/toolchain download. Git is needed only if you choose to clone.

Clone the repository:

```powershell
git clone --depth 1 https://github.com/TheWoovee/BareLine.git
cd BareLine
```

Alternatively, use **Code → Download ZIP** on [GitHub](https://github.com/TheWoovee/BareLine), extract it, and open PowerShell in the extracted folder containing `Cargo.toml`. A source ZIP can be built without Git installed; bundled native sources are included.

Build and run the preview:

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked -p bareline --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\bareline.exe
```

For an incremental development build on an x64 MSVC host:

```powershell
cargo run --locked -p bareline
cargo run --locked -p bareline -- --software
cargo run --locked -p bareline -- "C:\notes\todo.txt"
```

The development executable is `target\debug\bareline.exe`; run it directly when you only need to relaunch. If Cargo cannot find `link.exe` or the C++ compiler, check the C++ workload and SDK installation and try a Visual Studio developer terminal.

Building the editor does not require building the extension host or WASI components. Public packaging has additional tools and inputs; see [Windows packaging](packaging/windows/README.md). See [CONTRIBUTING.md](CONTRIBUTING.md) for focused checks and contribution guidelines.

## Known issues in this preview

These are current limitations of the preview builds. They are tracked for fixing; release notes say when one changes.

- **Network locations.** Files on network shares and mapped drives open only through **Open Remote File with Permission** and cannot be saved in place yet; use **Save Copy** to keep edits in a local folder. Removable drives, non-NTFS volumes (exFAT, FAT32, ReFS), OneDrive folders, junctions, and hard-linked files open and save, with a notice when saving there is weaker than on local NTFS.
- **Legacy encoding detection is a best guess.** GBK, Big5, Shift-JIS, EUC-JP and EUC-KR are detected only when the first 64 KiB contain enough characters to tell them apart. Short or mixed samples open as Windows-1252 with an "encoding may be wrong" hint naming the likely encodings. Use **Encoding > Interpret As** to reinterpret the original bytes; the bytes on disk are not changed until you save.
- **Compare limits.** In the normal editor, Compare is exact up to 200,000 lines and 64 MiB of combined text, with 5 seconds for each step. A region that exceeds a step's time or memory limit is shown as one changed block between the matching lines around it; larger inputs are shown as one changed block for the whole file. The status then reads "Coarse comparison". Byte-identical documents always show "No differences".
- **Large-file compare with many distinct lines.** When a large-file (paged) comparison has more distinct lines than its memory limit can index, it aligns the files on a sample of those lines, and a change between two sampled lines can show as one larger changed block. The status then reads "Coarse comparison (memory limit)".
- **Highlighting after several quick edits.** When several edits land before the editor redraws (for example fast typing while a frame is slow), highlighting restarts from the start of the file and the edited text can briefly show without colors. In the normal editor, a single edit, or a scroll back up, re-highlights from just before the visible text.
- **Large-file line numbers.** In large-file mode, line numbers are estimated until a background count of the file finishes; the status bar shows "Line numbers estimated · indexing" with its progress meanwhile. The count was rewritten for this preview and its duration on multi-gigabyte files has not been re-measured. Hiding lines in a large file can delay typing until the hidden range is mapped.
- **Regular expressions with anchors on text over 64 MiB.** A regular expression that uses `^`, `$`, `\b`, `\B`, `\A`, `\G`, `\X`, look-behind or backtracking verbs is matched against the whole text as context, which is limited to 64 MiB. In a document, or a file searched by Find in Folder or Replace in Files, larger than that, such a search stops and reports incomplete results ("Regex context exceeds 64 MiB"). Patterns without those items are searched window by window and are not affected.
- **Clipboard size.** Copy, cut, and paste through the Windows clipboard are limited by the `clipboard.max_bytes` setting (1 GiB by default); a larger selection is refused with a "larger than the clipboard limit" message and the clipboard is unchanged. Search and other single-line fields accept at most 16 KiB of pasted text.
- **Case-insensitive regular expressions** fold one character to one character: with Match case off, `strasse` finds `STRASSE` but not `Straße`. Literal and Extended search also fold `ß` to `ss`.
- **Screen-reader text of very large ranges.** A screen reader that asks for the text of a range wider than 1 MiB receives its first 1 MiB. On a large paged file, a part that is still loading after a short wait is left off the end of that text. Moving by line travels at most 1 MiB per request.
- **Run (F5) refuses some values.** Run accepts an absolute path or a program name found on `PATH`, never in the current folder, and shows the resolved program before it starts. Notepad++ variables such as `$(FULL_CURRENT_PATH)` are quoted for the program that receives them, and a value that cannot be passed literally is refused, for example text containing `%`, `!` or `"` for `cmd.exe`. `$(CURRENT_WORD)` is unavailable in large files, and a Run line cannot contain literal `${...}` text such as PowerShell's `${env:PATH}`.
- **Some settings have no effect yet.** These rows are shown in Settings and stored in `settings.toml`, but the editor does not apply them: **Auto-save interval**, **Keep a backup copy on save**, **When a file changes outside Bareline**, **Confirm before closing unsaved files** (Bareline always asks before closing unsaved changes), the Search defaults (**Match case by default**, **Whole word by default**, **Regular expressions by default**, **Wrap around at the end**, **Maximum results**), **Auto indent**, **Close brackets and quotes**, **Cursor shape**, **Scroll past the last line**, **Document map**, **Pinned tabs first** and **Chord timeout**. Use the Find panel options, the **File > Document** commands for external changes, and **View > Panels > Toggle Document Map** instead.
- **Spell check** checks only the normal editor in the first pane of a split; large-file mode and the second pane are not checked yet, and misspellings are not reported to screen readers.

## Preview limitations

- This is an unsigned development preview. Windows 10/11 clean-machine qualification, physical accessibility and IME testing, and final recovery/release acceptance remain pending. Keep independent backups of important files.
- Windows x64 is the available native application. Portable core checks on other operating systems do not imply a supported desktop app there.
- Updates, extension downloads, and extension execution are disabled in the default build. Bareline 1.0 ships without third-party plugins; JSON and XML tools and a Hex View are built in instead.
- Large-file operations, regex searches, comparisons, imports, and conversions have resource limits. A cancelled, limited, or unsupported operation must not be read as a complete result.
- Encoding detection is advisory for legacy text. Stateful encodings are unsupported, and conversion can refuse bytes or characters that cannot be represented in the chosen encoding.
- Notepad++ preference, user-language, and function-list import is selective. It is not plugin or full configuration compatibility.
- The user interface is in English only. Command and menu text goes through a resource table so that language packs can be added later.

Please include the Bareline version and build commit (**Help → About Bareline → Copy diagnostics**), Windows build, renderer, reproduction steps, and a small non-sensitive sample in [bug reports](https://github.com/TheWoovee/BareLine/issues); the diagnostics logs are in the `diagnostics` folder of your profile (see [Settings and local data](#settings-and-local-data)). Use the [security policy](SECURITY.md) for security reports.

## Repository layout and licensing

| Path | Purpose |
| --- | --- |
| `apps/bareline` | Windows editor application and native feature integration. |
| `crates` | Document, file I/O, editing, UI, search, syntax, settings, comparison, platform, and shared libraries. |
| `native/lexilla-bridge` | C++ syntax bridge and bundled Lexilla/Scintilla sources. |
| `apps/extension-host`, `extensions` | Separate extension runtime and first-party components. |
| `apps/update-helper` | Update helper for configured releases. |
| `packaging`, `release`, `scripts` | Packaging, public release configuration, and build utilities. |
| `tests`, `xtask` | Automated tests, fixtures, and development tools. |

The core is licensed under [MPL-2.0](LICENSE). SDK/protocol and applicable extension sources use [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), as marked in their source headers; see [LICENSE-SDK](LICENSE-SDK). Documentation follows the code it describes: Bareline's own Markdown files and `docs/` are MPL-2.0, except the extension walkthrough in `docs/extensions`, which is MIT OR Apache-2.0 like the SDK. The Bareline name, logo and icon are covered by the [trademark policy](TRADEMARKS.md), not by these licenses. Bundled third-party code retains its own licenses. Distribution packages include their third-party notices.
