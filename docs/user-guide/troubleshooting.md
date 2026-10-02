# Troubleshooting

## Bareline does not start

- **Windows warns about an unknown publisher.** Preview builds are not Authenticode-signed. Compare the file's SHA-256 checksum with the one published on the release page before you run it ([Install and run](../../README.md#install-and-run)).
- **`MSVCP140.dll` or `VCRUNTIME140.dll` is missing.** Only the first two previews (`v0.1.0-preview.20260928` and `.1`) need the Microsoft Visual C++ Redistributable. Newer previews include the runtime.
- **An error message appears at startup.** Bareline shows startup failures in a message box together with the location of its diagnostics folder, instead of exiting silently. Include that text in a bug report.
- **Another Bareline is using the profile.** One profile serves one Bareline process at a time; a second launch hands its files to the running window. If no window is visible, check the taskbar notification area (**Window > Tray > Keep Running in Tray** keeps Bareline there), or start a separate window with `--new-instance`.
- **Settings were reset.** A damaged, oversized or UTF-16 `settings.toml` is set aside or converted at startup and Bareline starts with defaults; a notice says what happened and where the old file is. To reset settings yourself, close Bareline and rename `settings.toml` in your profile.

## Display problems

- Bareline draws in software by default. To try GPU drawing, start with `--hardware` or set **Drawing mode** to hardware; `--software` goes back. If the graphics device is lost, Bareline recreates it, and falls back to software drawing if the device keeps failing.
- A drawing error is reported once as a notice instead of repeated dialogs. Include its text in a bug report.
- On a computer without Cascadia Mono, the editor font falls back to Consolas and then Courier New.

## Files and text

| Symptom | See |
| --- | --- |
| A file on a network share cannot be opened or saved | [Known issues: Network locations](../../README.md#known-issues-in-this-preview). Use **Open Remote File with Permission** and **Save Copy**. |
| Text shows wrong characters | [Encodings](encodings.md): use **Encoding > Interpret As**. |
| A file opened read-only with a binary notice | Choose **Edit as text** in the notice if the file is text. |
| A file failed to open | The tab keeps the error and offers **Retry** and **Open Read-Only (Large-File Mode)**. |
| Line numbers say "estimated" | [Large files](large-files.md#line-numbers). |
| A search says "Incomplete" | [Search limits](search-and-regex.md#limits). |
| Compare says "Coarse comparison" | [Compare](compare.md#how-large-inputs-are-compared). |
| A setting seems to have no effect | Some settings are not applied yet; see [Known issues](../../README.md#known-issues-in-this-preview). |
| Work may have been lost | Stop and follow [If you might have lost work](../../SUPPORT.md#if-you-might-have-lost-work) before starting Bareline again. |

## Diagnostics logs

Bareline keeps local logs in the `diagnostics` folder of your profile (`%LOCALAPPDATA%\Bareline\diagnostics`, or `<executable folder>\data\diagnostics` for a portable copy):

| File | Content |
| --- | --- |
| `bareline.log` | Startup, first frame, idle memory and other events, with the version and build commit. At most 512 KB. |
| `bareline.previous.log` | The previous `bareline.log` after it reached 512 KB. |
| `bareline.crash.log` | After a crash: the source-code location of the failure and whether queued recovery work was sealed. **Every start overwrites it**, so copy it before you start Bareline again. |

The logs are designed to hold events, versions, timings, memory figures and source-code locations, never document text or file paths. Nothing is uploaded; see the [privacy policy](../../PRIVACY.md). Review a log before attaching it to a public issue.

## Reporting a bug

1. Open **Help > About Bareline** (**F1**) and choose **Copy diagnostics**. The copied text holds the version, the build commit, the architecture, the renderer, the profile mode and whether logging is on, and no file paths or document text. `bareline.exe --version` prints the version and build commit too.
2. Note the Windows version and build from `winver`, for example `24H2 (OS Build 26100.4061)`.
3. Write down the exact steps from launch, what you expected and what happened.
4. If a file is needed, make a small synthetic sample that reproduces the problem, and state its size, encoding and line endings.
5. Open an issue with the matching form on [GitHub Issues](https://github.com/TheWoovee/BareLine/issues/new/choose): **Bug report**, **Data loss or crash**, **Notepad++ parity request**, **Feature request** or **Question**. Report vulnerabilities privately as described in [SECURITY.md](../../SECURITY.md).

The build commit lets the maintainer find the exact source of your build. Tagged previews show a version like `0.1.0-preview.<n>`; builds made from an untagged checkout show `0.1.0-dev+<commit prefix>`. [SUPPORT.md](../../SUPPORT.md) describes what is supported and what to include.
