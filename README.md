# Bareline

Bareline is a native Windows text and code editor written in Rust, with tabbed documents, split views, large-file editing, search and replacement, syntax highlighting, comparison, macros, and recovery.

**Status: Windows x64 preview.** The application is usable for evaluation, but compatibility, accessibility, recovery, and release qualification are still in progress. Preview packages are unsigned. Automatic updates, extension downloads, and extension execution are disabled in the default build.

[Download the Windows preview](https://github.com/TheWoovee/BareLine/releases) · [Report a bug](https://github.com/TheWoovee/BareLine/issues) · [Contribute](CONTRIBUTING.md)

[Code signing policy](CODE_SIGNING.md) · [Privacy policy](CODE_SIGNING.md#privacy-policy)

## Install and run

Bareline currently targets **64-bit Windows 10 22H2 (build 19045) or later, including Windows 11**. The installer refuses older Windows builds. This is a target compatibility floor; final clean-machine qualification remains pending. Native Linux, macOS, ARM64, and 32-bit releases are not available.

The executables include the C and C++ runtime, so no Microsoft Visual C++ Redistributable is needed; packaging rejects a build that imports it. The first two previews (`v0.1.0-preview.20260928` and `v0.1.0-preview.20260928.1`) still required the **Microsoft Visual C++ v14 Redistributable (x64)**; if one of those reports a missing `MSVCP140.dll`, `VCRUNTIME140.dll`, or `VCRUNTIME140_1.dll`, install that redistributable from [Microsoft's official download page](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) or use a newer preview.

Open the [Releases page](https://github.com/TheWoovee/BareLine/releases) and read the notes for the selected preview. Release assets and their checksums are listed there. GitHub's automatically generated **Source code** archives contain source, not a ready-to-run application.

### Portable ZIP

1. Download the Windows x64 portable ZIP from a release.
2. Extract the entire archive into a writable folder.
3. Run `bareline.exe`.

Keep the empty `bareline.portable` file next to the executable. It makes Bareline store settings, sessions, recovery, and other profile data in the adjacent `data` folder. Move that folder with the application if you want to keep your profile. Do not run directly from inside the ZIP.

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
| Files and tabs | Limited | New/open/save, Save As, Save Copy, Save All, read-only documents, restore the last closed tab, tab reordering, pinning, colors, sorting, vertical tabs, and a recent-document switcher. |
| Editing | Limited | Undo/redo, multiple carets, select next/all occurrences, rectangular editing, a column editor, plain-text paste, and optional in-memory clipboard history. |
| Text operations | Usable | Duplicate/move/join/split lines, case conversion, indentation, tabs/spaces conversion, whitespace trimming, line sorting including numeric sort, duplicate removal, and empty/blank-line removal. |
| Navigation | Usable | Go to line, bookmarks, hide/show selected lines, a document list, document map, and language-aware outline. |
| Split views | Usable | Horizontal or vertical splits, clone or move a document to the other view, separate view focus, and synchronized horizontal/vertical scrolling. |
| Search | Limited | Literal, extended, and PCRE2 regular-expression modes; case and whole-word options; search in a selection, current document, open documents, or folder; next/previous results; and five mark styles. |
| Replacement | Limited | Current-document replacement plus file/workspace replacement with a reviewable preview, selected changes, preserve-case option, optional backups, cancellation, and backup restoration. |
| Languages | Usable | Built-in syntax highlighting, language selection and file associations, comments, folding, completion, static signatures, and import/export of user-defined language definitions. |
| Workspace | Usable | Folder tree, refresh and incremental loading, file/folder creation, rename, retained deletion with undo, and document/outline panels. |
| Comparison | Limited | Side-by-side document comparison, compare with disk or a saved version, next/previous difference, copy a difference or selected range between sides, whitespace/case/EOL options, and configurable difference colors. |
| Encodings | Limited | Unicode and legacy encodings, reinterpret original bytes, convert the save encoding, control BOM output, and convert LF/CRLF/CR endings for a document or selection. |
| Recovery and sessions | Early | Session restoration, background recovery journals, a Recovery Center, recovery retry, and saving recovered documents to another file. |
| External changes and logs | Usable | External-change checks with keep/reload choices, optional automatic reload for clean local files, and read-only following of appended content with pause/resume controls. |
| Macros and external tools | Limited | Record, name, save, import/export, and replay macros; repeat a macro a fixed number of times or to end of file; assign shortcuts; run explicitly authorized external commands with captured output. |
| Utilities | Usable | Document statistics, MD5/SHA-1/SHA-256/SHA-512 hashes, Base64 and URL encode/decode, syntax-colored HTML/RTF export, and printing with font, margin, line-number, header/footer, and color options. |
| Customization | Usable | Light/dark/system themes, theme color overrides, fonts, wrapping, indentation, line numbers, whitespace display, a configurable toolbar, shortcut mapping, and user/workspace settings. |
| Rendering | Usable | Native Windows hardware rendering and a software renderer selectable at launch. |

Maturity: **Usable** areas have no known blocking issue in this preview. **Limited** areas work within the current limitations listed under [Known issues in this preview](#known-issues-in-this-preview). **Early** areas are implemented but their acceptance testing is still pending; keep independent backups.

MD5 and SHA-1 are provided for legacy integrity workflows; use an appropriate modern algorithm for security-sensitive work.

### Languages and encodings

The built-in language catalog includes **C, C++, C#, Java, JavaScript, TypeScript, Python, Rust, Go, HTML, CSS, JSON, XML, SQL, and TOML**. Language features depend on the selected language and available definitions. Completion and signatures are local editor features; this preview does not provide a language-server or debugger integration.

Supported text encodings are UTF-8, UTF-16 LE/BE, UTF-32 LE/BE, ISO-8859-1, Windows-1250 through Windows-1258, Shift-JIS, GBK, Big5, EUC-JP, and EUC-KR. Stateful encodings are unsupported. Detection samples up to 64 KiB and falls back to Windows-1252 for ambiguous legacy input; choose the encoding explicitly when needed. See the [codec catalog](crates/file-io/src/codecs/CATALOG.md) for aliases and byte-preservation behavior.

### Large files

Files up to **256 MiB** use the resident editor by default; larger files use paged editing. Defaults are 1 MiB pages, a 64 MiB cache per paged document, a 256 MiB aggregate document cache, and a separate 128 MiB undo RAM budget. Undo retains at most 100,000 changes per document. These limits are configurable in Settings.

These are resource defaults, not a guaranteed maximum file size or performance claim. File size, available memory, free disk space, encoding, and the operation affect what can complete. Encoding conversion can use temporary disk storage, with a default quota of 20 GiB. Search and comparison have bounded work and memory limits; unsupported streaming patterns or exhausted limits may prevent a complete result.

### Extensions and updates

The repository includes a separate extension host and first-party JSON, XML, and hex-view components. **They are not enabled in the default preview**, even if an extension preference is enabled in Settings. Their download and execution paths require a configured release with reviewed public trust keys, publisher identity, signed metadata, and a separately supplied runtime. This also applies to the in-app update pipeline. Do not treat checked-in test trust fixtures as production configuration.

## Everyday shortcuts

These are the defaults; use **Shortcut Mapper** from the Command Palette to inspect or change bindings.

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

## Command line

```powershell
.\bareline.exe "C:\notes\todo.txt"
.\bareline.exe --line 42 --column 5 "C:\src\main.rs"
.\bareline.exe --monitor "C:\logs\app.log"
.\bareline.exe --software --no-session --new-instance
```

| Option | Purpose |
| --- | --- |
| `--line N`, `--column N` | Open at a one-based location. `--column` requires `--line`. |
| `--read-only` | Open documents read-only. |
| `--monitor` | Open read-only and follow new content. |
| `--no-session` | Skip restoring the previous session. |
| `--no-extensions` | Disable extensions for the launch; default preview builds already disable extension execution. |
| `--new-instance` | Start a separate instance. |
| `--software`, `--hardware` | Select the renderer; choose one. |
| `--help` / `-h`, `--version` / `-V` | Show command-line help or the version. |
| `--` | Treat following arguments as file paths, including names beginning with a hyphen. |

A launch accepts up to 16 file paths; a launch with more paths is refused. Opening runs in the background. Launching without paths opens an Untitled document and may restore the saved session. Internal diagnostic modes are intended for development and are separate from ordinary document launches.

## Settings and local data

Open **Settings** from the Command Palette. The editor exposes common preferences in its settings UI and can open the underlying TOML file for more detailed editing.

| Launch type | Profile location |
| --- | --- |
| Installed or ordinary source-built executable | `%LOCALAPPDATA%\Bareline` |
| Portable executable with adjacent empty `bareline.portable` marker | `<executable folder>\data` |

The profile contains `settings.toml`, `keymap.toml`, `session.json`, and folders for recovery, macros, extensions, and diagnostics as those features are used. Older profiles under `%APPDATA%\Bareline` are handled by the profile migration path; `%APPDATA%` is also the fallback if `LOCALAPPDATA` is unavailable.

Workspace preferences live in `.bareline\settings.toml` under the workspace folder and are ignored until you opt in. The clipboard history setting is off by default and, when enabled, retains a bounded history only in memory for that session. Recovery journals and session files are local application data; preserve the profile when moving or replacing a portable installation.

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

- **Removable, cloud-synced, and network locations.** Opening and saving are currently refused on USB and other removable drives, non-NTFS volumes (exFAT, FAT32, ReFS), network shares and mapped drives, and folders redirected through reparse points, including OneDrive-redirected Documents and Desktop folders. Work on a local NTFS folder for now.
- **GBK and Big5 detection.** GBK text can be detected as EUC-KR and Big5 text as Windows-1252, which displays garbled characters. Use the encoding command to reinterpret the original bytes as GBK or Big5; the bytes on disk are not changed until you save.
- **Compare above 2,048 lines.** Above 2,048 lines or 1 MiB of text, or when the comparison exceeds its time budget, Compare shows the whole file as one changed block, even for identical files.
- **Large-file line indexing.** Very large paged files open and scroll immediately, but indexing their lines can take many minutes (a 300 MB log took more than 15 minutes in testing), and the indexing progress text can overlap document text.
- **Clipboard size.** Copy, cut, and paste through the Windows clipboard are limited to 4 MiB; a larger selection is refused with "Selection exceeds the clipboard limit" and the clipboard is unchanged.
- **Regular-expression search and replace on large files.** Find All, Count, and Replace All with a regular expression can stop at the search time limit on files of several megabytes. Treat a result that stopped at a limit as incomplete.
- **Run (F5)** requires an absolute program path, such as `C:\Windows\System32\where.exe`; programs are not looked up on `PATH`.
- **Launching with more than 16 paths** is refused and opens none of them.

## Preview limitations

- This is an unsigned development preview. Windows 10/11 clean-machine qualification, physical accessibility and IME testing, and final recovery/release acceptance remain pending. Keep independent backups of important files.
- Windows x64 is the available native application. Portable core checks on other operating systems do not imply a supported desktop app there.
- Updates, extension downloads, and extension execution are disabled in the default build. JSON/XML highlighting is built in; the separate JSON/XML tools and hex-view extension are not enabled preview features.
- Large-file operations, regex searches, comparisons, imports, and conversions have resource limits. A cancelled, limited, or unsupported operation must not be read as a complete result.
- Encoding detection is advisory for legacy text. Stateful encodings are unsupported, and conversion can refuse bytes or characters that cannot be represented in the chosen encoding.
- Notepad++ preference, user-language, and function-list import is selective. It is not plugin or full configuration compatibility.

Please include the Bareline version and build commit (**Help → About Bareline → Copy diagnostics**), Windows build, renderer, reproduction steps, and a small non-sensitive sample in [bug reports](https://github.com/TheWoovee/BareLine/issues). Use the [security policy](SECURITY.md) for security reports.

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

The core is licensed under [MPL-2.0](LICENSE). SDK/protocol and applicable extension sources use [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), as marked in their source headers; see [LICENSE-SDK](LICENSE-SDK). Bundled third-party code retains its own licenses. Distribution packages include their third-party notices.
