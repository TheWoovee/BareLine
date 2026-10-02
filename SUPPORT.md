# Support

Bareline is a Windows x64 preview with one maintainer (see [MAINTAINERS.md](MAINTAINERS.md)). Support is best effort through GitHub; there is no paid or commercial support and no guaranteed response time for ordinary bugs or questions. Security reports have an acknowledgement target in [SECURITY.md](SECURITY.md).

## Where to ask

| You want to | Use |
| --- | --- |
| Report a bug | The **Bug report** form on [GitHub Issues](https://github.com/TheWoovee/BareLine/issues/new/choose). |
| Report lost or damaged work, or a crash | The **Data loss or crash** form. Read [If you might have lost work](#if-you-might-have-lost-work) first. |
| Ask for a Notepad++ behavior or feature | The **Notepad++ parity request** form. |
| Suggest another feature or change | The **Feature request** form. |
| Ask a usage question | The **Question** form. Check the [README](README.md) and existing issues first. |
| Report a vulnerability | [Private vulnerability reporting](https://github.com/TheWoovee/BareLine/security/advisories/new), as described in [SECURITY.md](SECURITY.md). Never in a public issue. |
| Report abusive behavior | See the [code of conduct](CODE_OF_CONDUCT.md#enforcement). |
| Contribute a fix | [CONTRIBUTING.md](CONTRIBUTING.md). |

GitHub Discussions is not enabled for this repository, so questions are tracked as issues.

> **TODO (owner):** decide whether to enable GitHub Discussions for questions and ideas; if enabled, update this table and `.github/ISSUE_TEMPLATE/config.yml`.

Issues, pull requests and their attachments are public. Do not post private documents, credentials, recovery journals, `session.json`, or paths and file names you consider private. Use a small synthetic sample that reproduces the problem instead.

## What is supported during the preview

| Supported | Not supported |
| --- | --- |
| The latest preview on the [Releases page](https://github.com/TheWoovee/BareLine/releases) and current `master` source. | Older previews. Update to the latest preview and check whether the problem remains. |
| 64-bit Windows 10 22H2 (build 19045) or later, including Windows 11. | Older Windows builds, ARM64 and 32-bit Windows, and native Linux or macOS desktop builds. |
| The per-user installer, the portable ZIP, and source builds made as described in the [README](README.md#build-from-source). | Modified builds, builds with test fixture trust enabled, and forks. |
| Software rendering (the default) and hardware rendering (`--hardware`). | Online updates, extension downloads and extension execution, which are disabled in the default preview. |

Behavior listed under [Known issues in this preview](README.md#known-issues-in-this-preview) is already tracked; add new information to the existing issue rather than opening a duplicate. Feature areas marked **Preview** or **Experimental** in the [feature table](README.md#features) are still being qualified; [docs/STATUS.md](docs/STATUS.md) shows what has been verified. Keep independent backups of important files while using a preview.

Security fixes target the latest preview only; see [SECURITY.md](SECURITY.md#supported-versions).

## Information to include

- **Version and build:** open **Help → About Bareline** (F1) and choose **Copy diagnostics**, then paste the text. It contains the version, build commit, architecture, renderer and profile mode, and no document content.
- **Windows build:** run `winver` and copy the version and OS build, for example `24H2 (OS Build 26100.4061)`.
- **Steps:** the exact sequence of edits, commands and file operations, starting from launch, with what you expected and what happened.
- **Sample:** a minimal synthetic file that reproduces the problem, if one is needed. State the file size, encoding and line endings if they matter.

## If you might have lost work

Bareline keeps recovery journals for unsaved documents in the `recovery` folder of your profile. Protect them before trying anything else:

1. **Do not uninstall Bareline or delete its profile.** Uninstalling keeps the profile, but deleting the profile folder removes recovery data. The profile is `%LOCALAPPDATA%\Bareline` for installed and ordinary source-built copies, and the `data` folder next to `bareline.exe` for a portable copy with a `bareline.portable` marker. Older profiles can also exist under `%APPDATA%\Bareline`.
2. **Copy the `recovery` and `diagnostics` folders** to a safe location outside the profile, for example with File Explorer, while Bareline is closed. Do this before starting Bareline again: every launch empties `bareline.crash.log`, which records the crash location and whether queued recovery work was sealed, and can rotate `bareline.log` into `bareline.previous.log`. Take any log you attach to an issue from this copy.
3. **Start Bareline normally.** When recovery checkpoints are found, a notification reports how many documents can be recovered. Open the **Recovery Center** from the Command Palette (**Ctrl+Shift+P**) to preview, restore, or **Save Recovered Document As**. **Open Recovery Folder** shows the folder in File Explorer.
4. **Save restored documents to a new file** before closing them, then compare them with the original files.
5. If nothing is offered, a restore fails, or the restored content is older than expected, open a **Data loss or crash** issue. Keep your copies of the recovery and diagnostics folders until the issue is resolved.

Recovery journals and `session.json` contain your document text and paths. Do not attach them to a public issue. The issue form asks only for a listing of file names, sizes and times, which you can produce in PowerShell without opening any journal:

```powershell
$root = "$env:LOCALAPPDATA\Bareline\recovery"   # portable: <executable folder>\data\recovery
Get-ChildItem -LiteralPath $root -Recurse -Force |
    ForEach-Object { '{0}  {1}  {2:u}' -f $_.FullName.Substring($root.Length), $_.Length, $_.LastWriteTime }
```

The local diagnostics logs are in the `diagnostics` folder of the same profile: `bareline.log`, `bareline.previous.log`, and `bareline.crash.log`, which records a crash location and whether queued recovery work was sealed. Restarting Bareline overwrites `bareline.crash.log`, so after a crash read it from the copy made in step 2, not from the profile. The logs are designed to hold events, versions, timings and source locations, not document text or paths. Review them before pasting.
