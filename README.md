# Bareline

Bareline is a native text and code editor for Windows, written in Rust. It is built not to lose your work or change bytes you did not edit: unsaved text is journaled in the background, files keep their exact bytes and line endings, and files too large to hold in memory still open and save. In the measurements below it starts in about 80 ms and idles at about 23 MB of memory, and it offers a Notepad++ shortcut preset and Notepad++ imports.

**Status: preview.** Bareline is in active development. Windows x64 preview builds are published on the [Releases page](https://github.com/TheWoovee/BareLine/releases). They are not code-signed, and automatic updates, extension downloads and extension execution are turned off in these builds. Keep independent backups of important files while you evaluate it.

Bareline is open source. Most of the code is under the Mozilla Public License 2.0, and the extension SDK is under MIT or Apache-2.0 (see [License](#license)).

[Download](#download) · [What's new](#whats-new-in-020) · [Report a bug](https://github.com/TheWoovee/BareLine/issues) · [Contribute](CONTRIBUTING.md) · [Security](SECURITY.md) · [Privacy](PRIVACY.md)

## Why Bareline

- **It keeps your work.** Edits to every open document, including Untitled ones, are written to recovery journals in the background. The journals are flushed when Windows logs off or shuts down, and if Bareline hits a fatal error it seals them before it exits. After a crash or forced shutdown, the **Recovery Center** offers each document so you can restore it, compare it with the file on disk, or save it elsewhere.
- **It keeps your bytes.** Bytes that are invalid or undefined in the file's encoding are kept as they are, and an untouched save writes the original bytes back. Automated tests check that all 256 byte values survive a save, an unrelated edit and an undo in every supported encoding. Regular-expression replacements keep CRLF line endings. Encoding detection recognizes GBK, Big5, Shift-JIS, EUC-JP, EUC-KR and UTF-16 without a byte order mark.
- **It opens huge files.** Files larger than 256 MiB open in a paged mode that reads them in 1 MiB pages and by default keeps at most 64 MiB of each file in memory. You can scroll, search, replace, compare, edit and save them while the lines are counted in the background.
- **It is fast and light.** Drawing is done in software by default, so no GPU device is created at startup; Direct2D drawing is available with `--hardware`. Worker threads start only when they are first needed. See [Indicative measurements](#indicative-measurements).
- **It suits Notepad++ users.** Bareline has a Notepad++ shortcut preset, Notepad++-style regular-expression options (`^` and `$` per line, **. matches newline**), and import of user-defined languages, function lists and selected preferences. **Run** (F5) accepts Notepad++ variables such as `$(FULL_CURRENT_PATH)` and `$(CURRENT_WORD)`, and the command line accepts Notepad++ spellings such as `-n<line>` and `-multiInst`.
- **The tools are built in.** Compare two documents, a document with a file on disk, or a document with its saved version. **Replace in Files** shows a preview first, keeps backups and a receipt for each job, and can roll a job back. JSON, XML and Hex tools, syntax highlighting for 66 languages, 41 encodings and spell checking through Windows are included. There are no plugins to install.
- **It is accessible.** The editor, tabs, panels and Settings report their names, roles and text to screen readers through Windows UI Automation.
- **It is private.** There is no account, analytics, telemetry or crash-report upload, and the editor does not use the network to open, edit, search or save files.
- **It is a hardened Windows program.** The editor is written in Rust. Its native code is the bundled Lexilla lexers and PCRE2. The executables link the C runtime statically, so no Visual C++ Redistributable is needed. They are built with Control Flow Guard and load the DLLs they import only from System32. The packaging scripts check these properties in every package.
- **It is tested.** The Rust workspace runs more than 1,600 automated tests. There are also seeded property tests for documents, codecs, recovery, search and settings, nine `cargo-fuzz` targets that run nightly, end-to-end and accessibility harnesses, and Python tests for the release tooling. Release downloads carry GitHub build-provenance attestations.

### Indicative measurements

A release build of the 0.2.0 code (commit `1e849e9`) and Notepad++ 8.9.8.1 x64 portable were measured in turn on the same computer: AMD Ryzen 5 3600, 128 GB RAM, NVMe SSD, Windows 11 Pro (build 26200), 100% scaling. The files were already in the disk cache. Launch figures are medians of 7 runs, and file opens are medians of 3 runs (2 for the 300 MB file). A file counts as open when the whole document is available. For Bareline, that is when the status bar shows the exact line count. These are indicative numbers from one computer, not a formal benchmark.

| Measurement | Bareline 0.2.0 | Notepad++ 8.9.8.1 |
| --- | ---: | ---: |
| Launch to main window visible | 80 ms | 270 ms |
| Idle private memory, empty document | 23.2 MB | 59.9 MB |
| Open a 6 MB file, 62k lines (time / memory) | 256 ms / 32.7 MB | 534 ms / 74.5 MB |
| Open a 50 MB log, 468k lines | 318 ms / 85.9 MB | 589 ms / 166.9 MB |
| Open a 10 MB single-line file | 235 ms / 31.8 MB | 818 ms / 180.8 MB |
| Open a 300 MB log, 2.8M lines, to the full line count | 2,171 ms / 100.3 MB (text shown after 225–286 ms) | 873 ms / 386.2 MB |
| Replace All, 468,100 matches in the 50 MB log | 376 ms | 1,625 ms |
| Select All and Copy, 6 MB | 164 ms | 250 ms |
| Save the 50 MB log after a 1-byte edit | 204–235 ms | 76–94 ms |

Notepad++ is faster at saving large files and at reaching the full line count of the 300 MB file. See [Known issues](#known-issues-in-this-preview).

## What's new in 0.2.0

Changes since the 0.1.0 previews:

- **Data safety.** Recovery journals are flushed on Windows logoff and shutdown, and sealed when Bareline hits a fatal error. Recovery finds journals left by other instances and by processes whose ID was reused. Undo and Redo keep working when a recovery journal cannot be created, and undo history is never cleared silently.
- **Correct regular expressions.** Searches handle CRLF: `$` matches before a CRLF, replacements keep CRLF line endings, and a new **. matches newline** option controls whether `.` crosses lines.
- **Large edits finish.** Replace All of hundreds of thousands of matches completes (468,100 matches in a 50 MB file in under 0.4 s). Copying multi-megabyte selections works: the old 4 MiB clipboard cap is gone, and the limit is now the `clipboard.max_bytes` setting, 1 GiB by default.
- **Large files.** The background line count was rewritten and keeps its index across edits, so a 300 MB file is fully counted in about 2 s, where the count previously did not finish. Selection, overtype and page-boundary editing in large-file mode were fixed, and a file that does not fit in memory falls back to large-file mode instead of failing.
- **Encodings.** ISO-8859, KOI8, Mac and OEM code pages were added, for 41 encodings in total. GBK, Big5 and UTF-16 without a byte order mark are now detected correctly.
- **Languages.** The complete Lexilla 5.5.3 lexer set is bundled: 66 languages instead of 15, with detection by file name and `#!` line.
- **Built-in tools.** JSON, XML and Hex tools are part of the editor, and new spell checking through Windows offers suggestions, **Add to Dictionary** and **Ignore All**.
- **Notepad++ parity.** A Notepad++ shortcut preset, offered on first run, and **Run** (F5) with Notepad++ variables. Named sessions, pinned recent files, **Close All** and **Close Others**, path commands, a horizontal scroll bar, a column selection mode and a tab overflow list.
- **Faster and lighter.** Software drawing by default and worker threads started on demand cut idle private memory from 57 MB to 23 MB. A 50 MB file opens in 0.3 s instead of 4.1 s.
- **Clearer behavior.** A binary file opens read-only with a notice instead of a blocking prompt. Errors are reported in plain language, a failed open keeps its tab with the error, a damaged settings file is repaired at startup, and startup never exits without a message.
- **Accessibility.** Banners, Settings controls and all tabs are exposed to screen readers. Selected rows have a cue that does not rely on color, and text ranges stay readable on long lines and across edits.
- **Security and distribution.** A static C runtime (no Visual C++ Redistributable), Control Flow Guard, DLL loading restricted to System32, the version and build commit in **Help > About**, build-provenance attestations for downloads, and safer quoting of values passed to programs by **Run**.

## Download

Bareline 0.2.0 is published as the preview [v0.2.0-preview.2](https://github.com/TheWoovee/BareLine/releases/tag/v0.2.0-preview.2). Download one of these files:

| File | Use |
| --- | --- |
| `bareline-0.2.0-windows-x64-setup.exe` | The installer. It installs for the current user by default and needs no administrator rights. |
| `bareline-0.2.0-windows-x64-portable.zip` | The portable copy. Extract it and run `bareline.exe`. Nothing is installed. |

The release also contains `SHA-256SUMS`, `PREVIEW-NOTES.md`, `LICENSE`, `THIRD-PARTY-NOTICES.md` and `SBOM.json`. GitHub's automatic **Source code** archives contain the source only, not a program you can run.

**Check the download.** `SHA-256SUMS` lists the SHA-256 checksum of every other file in the release. Compare the checksum of your download with its line there:

```powershell
Get-FileHash .\bareline-0.2.0-windows-x64-setup.exe -Algorithm SHA256
```

Every file in the release also has a build-provenance attestation from the GitHub workflow that built it. With the [GitHub CLI](https://cli.github.com/), this command checks that a file was built by the repository's release workflow:

```powershell
gh attestation verify .\bareline-0.2.0-windows-x64-setup.exe --repo TheWoovee/BareLine
```

**Windows SmartScreen.** Preview builds are not code-signed, so Windows may show **Windows protected your PC** or an unknown-publisher warning. Check the checksum or the attestation first, then choose **More info > Run anyway**. See [Unsigned builds and SmartScreen](#unsigned-builds-and-smartscreen).

## Features

- **Large files are paged.** Files larger than 256 MiB (the **Large-file threshold** setting, `document.resident_max_bytes`) open in large-file mode. Bareline reads them in 1 MiB blocks and by default keeps at most 64 MiB of each file in memory. You can scroll, search, replace, compare, edit and save them. Line numbers are estimated until a background count finishes.
- **Crash recovery.** Edits to open documents, including Untitled ones, are written to recovery journals in your profile in the background. After a crash or forced shutdown, the **Recovery Center** offers each document so you can restore it, compare it with the file on disk, or save it elsewhere.
- **Built-in compare.** Compare two documents, a document with a file on disk, or a document with its last saved version. You can step through the differences and copy changes from one side to the other.
- **Search modes.** Search has three modes: Literal, Extended (escape sequences such as `\n` and `\t`), and Regular expression. Regular expressions use PCRE2. `^` and `$` match at line boundaries, and `.` matches a line break only when **. matches newline** is on. You can search the selection, the current document, all open documents (**Find in Open Documents**) or a folder (**Find in Folder**).
- **Replace in Files with backups.** Folder-wide replacement first shows a preview in which you choose which changes to apply. Each job keeps backups and a receipt, so you can roll it back later. A job that a crash interrupted is reconciled at the next start.
- **Encodings.** 41 encodings: UTF-8, UTF-16 LE/BE, UTF-32 LE/BE, ISO-8859-1 to ISO-8859-8, ISO-8859-10 and ISO-8859-13 to ISO-8859-16, Windows-874 and Windows-1250 to Windows-1258, KOI8-R, KOI8-U, Mac Roman, Mac Cyrillic, OEM 437, 850, 852 and 866, Shift-JIS, GBK, Big5, EUC-JP and EUC-KR. Bareline detects the encoding and byte order mark, can reinterpret a file's original bytes in another encoding (**Encoding > Interpret As**), can convert the save encoding, and converts line endings (LF, CRLF and CR).
- **Syntax highlighting for 66 languages.** Highlighting comes from the bundled Lexilla lexers, with comment toggling, folding, an outline and local completion. You can also import user-defined languages.
- **JSON, XML and Hex tools.** These are built in under **Tools > JSON, XML and Hex**. You can format, minify and validate JSON, format and validate XML, run XPath queries, and view a file's original bytes in a read-only Hex View.
- **Spell check.** Bareline uses the Windows spell-checking API and your Windows language dictionaries. Plain text and Markdown are checked. Code is not checked unless you turn it on for that language, and then only comments and strings are checked.
- **Notepad++ shortcuts.** A Notepad++ shortcut preset is included, and you can import keymaps and some Notepad++ settings.
- **Accessibility.** The editor, tabs, panels and Settings report their names, roles and text to screen readers through Windows UI Automation (via AccessKit).
- **No telemetry.** There is no account, analytics, telemetry or crash-report upload. Preview builds make no update requests. See the [privacy policy](PRIVACY.md).
- **Portable mode.** A portable copy keeps all its data in a folder next to the executable.

Bareline also has multiple carets, rectangular selection with a column editor, bookmarks, split views, sessions, macros, an external **Run** command, document statistics and hashes, HTML/RTF export and printing. Search for any command by name in the **Command Palette** (Ctrl+Shift+P).

## System requirements

- **Windows 10 22H2 (build 19045) or later, including Windows 11, 64-bit (x64).** The installer refuses older builds and computers that cannot run x64 programs (`MinVersion=10.0.19045`, `ArchitecturesAllowed=x64compatible` in [`packaging/windows/bareline.iss`](packaging/windows/bareline.iss)). The portable ZIP does not check the Windows version, and older builds are untested and unsupported. Windows 11 on Arm can run the x64 build, but this has not been tested.
- No Visual C++ Redistributable is needed, because the C and C++ runtime is linked statically.
- No builds are available for Linux, macOS, ARM64 or 32-bit Windows.

## Install and run

Download the installer or the portable ZIP as described in [Download](#download), and check it against `SHA-256SUMS` before you run it. Later previews are listed on the [Releases page](https://github.com/TheWoovee/BareLine/releases). Their files follow the same pattern, `bareline-<version>-windows-x64-setup.exe` and `bareline-<version>-windows-x64-portable.zip`.

### Installer

The installer installs Bareline for the current user by default, which needs no administrator rights. You can choose an all-users installation instead, and Windows then asks for elevation. Two optional tasks are offered, both off by default:

- **Add Open with Bareline to Explorer**
- **Register Bareline as an available text editor.** This does not change your default programs.

### Portable ZIP

1. Extract the whole archive into a writable folder. Do not run Bareline from inside the ZIP.
2. Run `bareline.exe`.

Bareline runs in portable mode when an empty file named `bareline.portable` is next to `bareline.exe`, and the ZIP includes this file. In portable mode, settings, sessions, recovery journals and other profile data are stored in a `data` folder next to the executable. If that folder cannot be written, for example on write-protected media, Bareline shows a warning and does not save the session. It keeps recovery journals under `%LOCALAPPDATA%\Bareline\portable-recovery` until the folder can be written again.

### Unsigned builds and SmartScreen

Preview executables are not Authenticode-signed. Windows SmartScreen may show **Windows protected your PC** or an unknown-publisher warning. Check the release source and the SHA-256 checksum before you choose **More info > Run anyway**. Builds published by the release workflow also carry GitHub build-provenance attestations, which you can check with `gh attestation verify <file> --repo TheWoovee/BareLine`. The [code signing policy](CODE_SIGNING.md) explains the current status.

Previews are updated by hand: download a newer release and follow its notes. The in-app updater is not available in preview builds.

### Settings and local data

| Launch | Profile folder |
| --- | --- |
| Installed copy, or a copy built from source | `%LOCALAPPDATA%\Bareline` |
| Portable copy (`bareline.portable` next to the executable) | `<folder of bareline.exe>\data` |

The profile holds `settings.toml`, `keymap.toml`, `session.json`, and the `recovery`, `macros` and `diagnostics` folders. The `recovery` folder holds the recovery journals and Replace in Files backups and receipts, and the `diagnostics` folder holds local logs. A profile from an older version in `%APPDATA%\Bareline` is migrated. If `LOCALAPPDATA` is not set, Bareline uses `%APPDATA%\Bareline`. Open **Settings** from the **Settings** menu or the Command Palette. From there you can also open the underlying TOML file. The [privacy policy](PRIVACY.md) lists everything Bareline keeps locally.

### Uninstall

Close Bareline first. To remove an installed copy, open **Settings > Apps > Installed apps** (Windows 11) or **Apps & features** (Windows 10), select Bareline and choose **Uninstall**. This removes the program files and the optional Explorer integration but keeps your profile, so your settings and recovery data survive a reinstall. To remove the profile too, delete `%LOCALAPPDATA%\Bareline` (and `%APPDATA%\Bareline` if it exists) after you have saved anything you need from it. To remove a portable copy, delete its folder. Its `data` folder can hold unsaved work, so check it first.

## Command line

```text
Usage: bareline [OPTIONS] [--] [FILE ...]

Opens up to 16 files on top of the restored session; File > Recent Files >
Open Remaining Command-Line Files opens the rest, and a launch handed to a
running window lists them as not opened. A file that does not exist opens as
a new document and is created when you save it. Use -- before file names
that begin with '-'.

Options:
  -                 Read standard input into a new Untitled document, for at
                    most 10 seconds. With no files, or when a running window
                    takes the files, the text opens in a separate window that
                    neither restores nor saves the session
  --line N          Go to line N (one-based) in the opened files
  --column N        Go to column N on that line (requires --line; the
                    Notepad++ -c<column> alone uses line 1)
  --read-only       Open the files read-only
  --monitor         Open read-only and follow changes to the files
  --no-session      Do not restore or save the previous session
  --no-extensions   Start without extensions
  --new-instance    Open a separate window instead of reusing a running one
  --software        Use software rendering (the default)
  --hardware        Use hardware (GPU) rendering
  -h, --help        Show this help
  -V, --version     Show the version

Notepad++ spellings are accepted: -n<line> -c<column> -ro -multiInst
-nosession -noPlugin; -notabbar is ignored.
```

This is the text that `bareline --help` prints. When there is no console to print to, Bareline shows the help in a message box. Examples:

```powershell
.\bareline.exe --line 42 --column 5 "C:\src\main.rs"
.\bareline.exe --monitor "C:\logs\app.log"
.\bareline.exe --hardware --new-instance --no-session
Get-Content .\notes.txt | .\bareline.exe -
```

`--software` and `--hardware` override the **Drawing mode** setting (`renderer.mode`) for one launch. `--no-session`, `--no-extensions` and `--new-instance` do not turn off crash recovery.

## Keyboard

Some of the default shortcuts are listed below. To see or change every binding, open **Settings > Shortcut Mapper**.

| Action | Shortcut |
| --- | --- |
| New / Open / Save / Save As | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S |
| Close tab / Restore last closed tab | Ctrl+W / Ctrl+Shift+T |
| Command Palette | Ctrl+Shift+P |
| Find / Replace / Find in Open Documents | Ctrl+F / Ctrl+H / Ctrl+Shift+F |
| Find next / previous | F3 / Shift+F3 |
| Go to line | Ctrl+G |
| Next / previous tab, recent-document switcher | Ctrl+PageDown / Ctrl+PageUp, Ctrl+Tab |
| Add caret above / below | Ctrl+Alt+Up / Ctrl+Alt+Down |
| Select next occurrence / all occurrences | Ctrl+D / Ctrl+Shift+L |
| Duplicate lines / Join lines / Move lines up, down | Ctrl+Shift+D / Ctrl+J / Alt+Up, Alt+Down |
| Toggle line comment / block comment | Ctrl+/ / Ctrl+Shift+/ |
| Toggle bookmark / next / previous | Ctrl+F2 / F2 / Shift+F2 |
| Column editor / Paste plain text | Alt+C / Ctrl+Shift+V |
| Go to / select to matching brace | Ctrl+B / Ctrl+Shift+B |
| Zoom in / out / reset | Ctrl+= / Ctrl+- / Ctrl+0 |
| Focus other split view / Full screen | F6 / F11 |
| Print / Run | Ctrl+P / F5 |

**Notepad++ preset:** choose **Settings > Import from Notepad++ > Use Notepad++ Shortcuts**, or set **Shortcut preset** to Notepad++ in the **Keyboard** section of Settings (`keyboard.preset`). With this preset, for example, Ctrl+D duplicates a line, Ctrl+Q toggles a comment and Ctrl+Shift+S saves all documents. Shortcuts you changed yourself are kept when you switch presets.

## Build from source

Bareline builds on Windows x64 with the MSVC toolchain.

Requirements:

- **Rust 1.98.1.** [`rust-toolchain.toml`](rust-toolchain.toml) pins it, with rustfmt and Clippy, and rustup installs it automatically. The workspace `rust-version` is also 1.98.1. Install Rust with [rustup](https://rustup.rs/), using the default `x86_64-pc-windows-msvc` host.
- **Visual Studio 2022 Build Tools** (or Visual Studio 2022) with the **Desktop development with C++** workload. This workload includes the MSVC x64/x86 build tools (`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`) and a Windows SDK. The build compiles the bundled Lexilla lexers (C++17) and PCRE2 (C) with `cc`. CI runs on GitHub's `windows-2022` image.
- Internet access the first time, to download the crates. Git is needed only to clone the repository: a source ZIP downloaded from GitHub builds with the same `cargo` commands. Packaging (`build-preview.ps1`) requires a Git checkout, because it records the commit.

Build and run:

```powershell
git clone https://github.com/TheWoovee/BareLine.git
cd BareLine
cargo build --release --locked -p bareline
.\target\release\bareline.exe
```

For development, use `cargo run --locked -p bareline` (the debug build is `target\debug\bareline.exe`). `bareline` is the workspace's default member. [`.cargo/config.toml`](.cargo/config.toml) links the C runtime statically and enables Control Flow Guard for the MSVC target.

Run the tests:

```powershell
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked
```

The [Correctness workflow](.github/workflows/ci.yml) is the reference for CI checks. It also runs the Python 3.12 scripts in `.github/workflows`, `scripts` and `tests`. CI compares Clippy warnings against a recorded baseline, so new warnings fail the check.

**Building the packages.** You need PowerShell 7, `cargo install --locked cargo-cyclonedx --version 0.5.9` for the SBOM and, for the installer, Inno Setup **6.4.3** exactly (the scripts reject other versions):

```powershell
pwsh -File .\packaging\windows\build-preview.ps1                    # portable ZIP
pwsh -File .\packaging\windows\build-preview.ps1 -Installer -Iscc "<path to ISCC.exe>"
```

The script builds an unsigned release of `bareline` and `bareline-update-helper` with default features off. It also generates `THIRD-PARTY-NOTICES.md` and `SBOM.json`, and writes the packages, `PREVIEW-NOTES.md` and `SHA-256SUMS` to `dist/preview`.

**Linux and macOS.** The application runs only on Windows. On Linux and macOS, `bareline` prints a message and exits. CI builds and tests the shared crates on `ubuntu-latest` and `macos-latest` to keep them platform-neutral. `crates/platform-linux` and `crates/platform-macos` are compile-only adapters, not ports.

### Building on Linux and macOS (preview)

This describes the port in progress on the `port-integration` branch, for contributors. It does not change which platforms the published builds support.

**Linux.** On Ubuntu 24.04 (CI uses `ubuntu-latest`), install the build packages and the X11 runtime libraries, then build with Cargo as on Windows:

```bash
sudo apt-get install -y libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libxrandr-dev libxkbcommon-x11-0 libxcursor1
cargo build -p bareline --release --locked
```

The smoke run below also uses `xvfb xauth scrot xdotool x11-utils at-spi2-core fonts-dejavu-core fonts-noto-cjk`.

**macOS.** Install the Xcode Command Line Tools (`xcode-select --install`) for the C and C++ parts, then run the same `cargo build`.

**Cross type-check for Apple silicon.** From Windows or Linux, crates without a C or C++ build step can be checked for macOS:

```bash
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin --locked -p bareline-platform-macos -p bareline-platform-posix -p bareline-renderer-soft
```

The whole editor cannot be checked this way: the Lexilla bridge and PCRE2 compile C and C++ and need the macOS SDK. The `macos-latest` runners build it instead.

**Smoke workflow.** [`port-smoke.yml`](.github/workflows/port-smoke.yml) runs on every push to `port-integration` and on manual dispatch, on `ubuntu-latest` (under Xvfb) and `macos-latest`. It builds the release editor, renders `soft_probe --offscreen`, and launches the editor with a sample file. It waits for the editor's window, then records the launch-to-window time and resident memory and takes a screenshot. Finally it quits the editor with SIGTERM, checks that the exit was clean, and runs the tests of the port crates. [`scripts/port_smoke.py`](scripts/port_smoke.py) does the launch and the measurements, and also runs locally (for example `xvfb-run -a python3 scripts/port_smoke.py --label linux --executable target/debug/bareline --sample <file> --evidence evidence --profile <scratch folder>`). To download the evidence (screenshots, `<os>-metrics.json`, editor output and JUnit test logs) with the GitHub CLI:

```bash
gh run list --workflow port-smoke.yml --branch port-integration --limit 5
gh run download <run-id> --name port-smoke-linux --dir port-smoke/linux
gh run download <run-id> --name port-smoke-macos --dir port-smoke/macos
```

When `packaging/macos/bundle.sh` exists, the macOS job also uploads the `.app` zip and `.dmg` as `port-smoke-macos-bundle`.

## Repository layout

| Path | Contents |
| --- | --- |
| `apps/bareline` | The Windows editor application: window, menus, panels and launch handling. |
| `apps/update-helper` | Update helper for configured releases. Its update functions are disabled in preview builds. |
| `apps/extension-host` | Separate Wasmtime/WASI extension host process. It is not included in preview builds. |
| `crates/document` | Text storage, including the paged large-file document model. |
| `crates/file-io` | Encodings, loading and saving, recovery journals, sessions and profile migration. |
| `crates/editor-surface` | Editing behavior: carets, selections and line and text transforms. |
| `crates/search` | Literal, extended and PCRE2 search, and Find/Replace in Files. |
| `crates/syntax` | Language catalog, highlighting through Lexilla, folding and outline. |
| `crates/diff` | Compare engine. |
| `crates/app`, `crates/ui`, `crates/commands`, `crates/settings` | Application model, view tree and widgets, command registry and keymaps, settings. |
| `crates/renderer`, `crates/renderer-recording` | Platform-neutral drawing operations, and a recording backend for tests. |
| `crates/platform`, `crates/platform-windows` | Platform interface and the Windows implementation, including software and Direct2D rendering, UI Automation, spell check and the clipboard. |
| `crates/platform-linux`, `crates/platform-macos` | Compile-only adapters used to keep shared code portable. |
| `crates/macros`, `crates/diagnostics`, `crates/distribution`, `crates/unicode-fold` | Macros and external commands, logging and crash handling, release and update contracts, Unicode case folding. |
| `crates/extension-sdk`, `crates/extensions-protocol` | Extension SDK and protocol (MIT OR Apache-2.0). |
| `extensions/` | JSON, XML and Hex components (`json-tools`, `xml-tools`, `hex-view`, `common`). The editor's built-in tools use them. |
| `native/lexilla-bridge` | C++ bridge to the bundled Lexilla and Scintilla sources. |
| `vendor/accesskit_windows-0.35.0` | Patched copy of `accesskit_windows`, substituted through `[patch.crates-io]` (see its `PATCHES.md`). |
| `packaging/` | Windows packaging (Inno Setup script, build, verification, notices and SBOM scripts) and a Scoop manifest. |
| `release/` | Release configuration schema, preview configuration and release-note templates. |
| `build-support/` | Code shared by the applications' build scripts. |
| `scripts/` | Release and packaging helpers and their Python tests. |
| `tests/` | End-to-end, accessibility, performance, soak, security-corpus and visual test harnesses. |
| `fuzz/` | `cargo-fuzz` targets, in a separate workspace that runs in the nightly fuzz workflow. |
| `xtask/` | Development tasks (`cargo xtask`), for example performance smoke tests. |

## Preview limitations

- Builds are not code-signed, so Windows SmartScreen may warn before the first launch (see [Unsigned builds and SmartScreen](#unsigned-builds-and-smartscreen)).
- Bareline runs only on 64-bit Windows (x64). There are no Linux, macOS, ARM64 or 32-bit builds.
- Clean-machine qualification on each supported Windows version, testing with physical screen readers and input method editors, and final recovery acceptance are still pending.
- The user interface is in English only.
- Bareline 1.0 does not load third-party plugins. The JSON, XML and Hex tools are built in. Updates, extension downloads and extension execution are turned off in preview builds.
- Notepad++ import covers selected preferences, user-defined languages and function lists. It does not make Notepad++ plugins work and does not import the full configuration.
- Large-file operations, regular-expression searches, comparisons, imports and conversions have resource limits. A cancelled or limited operation reports that its result is incomplete.

## Known issues in this preview

- **Saving large files is slower than in Notepad++.** A save writes a complete new copy next to the original and then replaces it. Saving a 50 MB file after a 1-byte edit takes about 0.2 s, about 2.5 times as long as Notepad++ (see [Indicative measurements](#indicative-measurements)). Saving a multi-gigabyte file takes time proportional to its size and needs free space for a second copy on the same volume.
- **Large-file line numbers.** In large-file mode, line numbers are estimated until the background count finishes, and the status bar shows its progress. Notepad++ reaches the full line count of a 300 MB file sooner, although Bareline shows the text first.
- **Network locations.** Files on network shares and mapped drives open only through **Open Remote File with Permission** and cannot be saved in place yet. Use **Save Copy** to keep edits in a local folder.
- **Legacy encoding detection is a best guess.** Detection reads the first 64 KiB. Short or mixed samples open as Windows-1252 with an "encoding may be wrong" hint, and you can use **Encoding > Interpret As** to choose another encoding. Stateful encodings are not supported. Characters that the target encoding cannot represent are refused, not replaced.
- **Compare limits.** Comparison is exact up to 200,000 lines and 64 MiB of combined text. Beyond that, or when a step runs out of time or memory, the differing region is shown as one changed block and the status reads "Coarse comparison". A large-file comparison with more distinct lines than its memory limit can index aligns the files on a sample of those lines, so a change between two sampled lines can show as one larger block. The status then reads "Coarse comparison (memory limit)".
- **Regular expressions on text over 64 MiB.** Patterns that use anchors, word boundaries, look-behind or backtracking verbs need the whole text as context, which is limited to 64 MiB. On larger text, such a search stops and reports "Regex context exceeds 64 MiB". Other patterns are searched window by window.
- **Case-insensitive regular expressions** fold one character to one character. With **Match case** off, `strasse` finds `STRASSE` but not `Straße`. Literal and Extended search also fold `ß` to `ss`.
- **Highlighting after several quick edits.** When several edits land before the editor redraws, for example fast typing while a frame is slow, highlighting restarts from the start of the file and the edited text can briefly show without colors.
- **Size limits.** The JSON and XML tools work on up to 16 MiB of text, and Hex View shows the first 1 MiB of a file. Clipboard transfers are limited by `clipboard.max_bytes` (1 GiB by default). Search and other single-line fields accept at most 16 KiB of pasted text.
- **Screen-reader text of very large ranges.** A screen reader that asks for the text of a range wider than 1 MiB receives only its first 1 MiB. In a large file, a part that is still loading after a short wait is left off the end of that text. Moving by line travels at most 1 MiB per request.
- **Run (F5) refuses some values.** Run starts a program given by an absolute path or found on `PATH`, and shows the resolved program before it starts it. Notepad++ variables are quoted for the program that receives them, and a value that cannot be passed literally is refused, for example text containing `%`, `!` or `"` for `cmd.exe`.
- **Spell check** covers only the normal editor in the first pane of a split. It does not check large-file mode or the second pane, and misspellings are not yet reported to screen readers.
- **A restored recovery stays listed after you save it.** **Restore** opens the recovered text as a separate document and keeps the recovery. After you save that document, the Recovery Center still lists the recovery at the next start until you choose **Delete**.
- **Some settings have no effect yet.** They are shown in Settings but not applied. They include **Auto-save interval**, **Keep a backup copy on save** and the Search defaults. Use the options in the Find panel instead.

Please include the text from **Help > About Bareline > Copy diagnostics**, your Windows build, the steps to reproduce and a small non-sensitive sample in [bug reports](https://github.com/TheWoovee/BareLine/issues). Local logs are in the `diagnostics` folder of your profile.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before you open a pull request. It covers the development setup, focused checks, the Developer Certificate of Origin sign-off (`git commit -s`) and the rules for AI-assisted changes. Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security, privacy and code signing

- **Security:** report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md). Do not report them in public issues.
- **Privacy:** [PRIVACY.md](PRIVACY.md) describes the data kept locally and the network use. The editor does not use the network to open, edit, search or save files.
- **Code signing:** [CODE_SIGNING.md](CODE_SIGNING.md) describes why preview builds are unsigned and the signing process planned for later.

## License

| License | Applies to |
| --- | --- |
| [MPL-2.0](LICENSE) | All Bareline files not listed in the next row, including `apps/`, the other crates in `crates/`, `native/lexilla-bridge` (the bridge code), `packaging/`, `scripts/`, `tests/`, `xtask/`, `fuzz/` and the documentation. This is the workspace `license` in `Cargo.toml`. |
| MIT ([LICENSE-MIT](LICENSE-MIT)) OR Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)), at your option | `crates/extension-sdk`, `crates/extensions-protocol` and everything in `extensions/`, as stated in [LICENSE-SDK](LICENSE-SDK) and in those crates' `Cargo.toml`. A file whose SPDX header names another license is covered by that license. |

Bundled third-party code keeps its own license: Lexilla and Scintilla (`native/lexilla-bridge/bundled/*/License.txt`), `vendor/accesskit_windows-0.35.0` (MIT OR Apache-2.0), the Unicode data in `crates/search/data` and `crates/unicode-fold/data` (`LICENSE-UNICODE`), and the DejaVu Sans Mono 2.37 font that `crates/renderer-soft` embeds (Bitstream Vera and Arev Fonts licenses, `crates/renderer-soft/fonts/LICENSE-DejaVu.txt`; `packaging/windows/generate-notices.ps1` adds it to the notices of any package that links that crate). License texts for PCRE2 and sljit, and for crates whose packages omit a license file, are under `packaging/windows/native-licenses` and `packaging/windows/license-overrides`.

**Third-party notices.** Release packages include `THIRD-PARTY-NOTICES.md`. [`packaging/windows/generate-notices.ps1`](packaging/windows/generate-notices.ps1) generates it from the locked dependency graph for `x86_64-pc-windows-msvc` and these license texts, and it fails if a license text is missing. `build-preview.ps1` runs it, and you can also run it on its own:

```powershell
cargo fetch --locked --target x86_64-pc-windows-msvc
pwsh -File .\packaging\windows\generate-notices.ps1 -OutputFile THIRD-PARTY-NOTICES.md
```

**Trademarks.** The Bareline name, logo and icon are not covered by these licenses. See [TRADEMARKS.md](TRADEMARKS.md). Forks must use a different name and mark.
