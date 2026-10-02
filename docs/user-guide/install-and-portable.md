# Install and portable mode

Bareline is available as a per-user installer and as a portable ZIP for 64-bit Windows. Both contain the same executables. Preview packages are not Authenticode-signed; check the SHA-256 checksum published with the release before running them (see [Install and run](../../README.md#install-and-run)).

## Supported Windows versions

| | |
| --- | --- |
| Target | 64-bit Windows 10 22H2 (build 19045) or later, including Windows 11. |
| What the installer enforces | `MinVersion=10.0.19045` and `ArchitecturesAllowed=x64compatible` in [`bareline.iss`](../../packaging/windows/bareline.iss): setup refuses older Windows versions and computers that cannot run x64 programs. Windows 11 on Arm can run x64 programs, so setup accepts it, but that combination is untested. |
| Portable ZIP | No version check. Older Windows builds are untested and unsupported. |
| Qualification | Clean-machine testing on the supported versions is still pending; see [STATUS.md](../STATUS.md). |

No Microsoft Visual C++ Redistributable is needed: the C and C++ runtime is linked into the executables.

## Installer

Run `bareline-<version>-windows-x64-setup.exe`. It installs for the current user by default (no administrator rights); the setup dialog and command line can choose an all-users installation instead. Two optional tasks are off by default:

- **Add Open with Bareline to Explorer** adds a context-menu entry.
- **Register Bareline as an available text editor** lists Bareline among the programs that can open text files. It does not change your default programs.

An installed copy keeps its profile in `%LOCALAPPDATA%\Bareline` and adds opened files to Windows Recent items unless you turn off **Add opened files to Windows Recent items** in Settings (see the [privacy policy](../../PRIVACY.md)).

To upgrade, run the setup of a newer preview; it replaces the application files and keeps your profile. Close Bareline first; setup offers to close it. Preview builds do not update themselves.

To uninstall, use **Settings > Apps > Installed apps** (Windows 11) or **Apps & features** (Windows 10). Uninstalling removes the application files and the optional Explorer entries and keeps your profile, including recovery journals.

## Portable ZIP

1. Extract the whole ZIP into a writable folder. Do not run Bareline from inside the ZIP.
2. Run `bareline.exe`.

The empty file `bareline.portable` next to `bareline.exe` switches on portable mode: settings, sessions, recovery journals, macros and diagnostics go to the `data` folder beside the executable, and nothing is added to Windows Recent items. Bareline checks only that the marker file exists; its content does not matter. Move the `data` folder together with the application to keep your profile.

If the `data` folder cannot be written, for example on write-protected media, Bareline shows a persistent warning, stops saving the session, and keeps recovery journals in `%LOCALAPPDATA%\Bareline\portable-recovery\<key>`, a folder of its own for each portable copy. Journals already on the media are offered again once the folder is writable.

The repository also contains a [Scoop manifest](../../packaging/scoop/bareline.json) for the portable ZIP. It is not yet published in a Scoop bucket. It removes the portable marker, so a Scoop installation keeps its profile in `%LOCALAPPDATA%\Bareline`.

## The profile

| Launch | Profile folder |
| --- | --- |
| Installed, or an ordinary source build | `%LOCALAPPDATA%\Bareline` |
| Portable, with `bareline.portable` beside the executable | `<executable folder>\data` |

The profile holds `settings.toml`, `keymap.toml`, `session.json`, the recent-files list, and the `recovery`, `macros`, `extensions` and `diagnostics` folders as those features are used. Profiles from older previews under `%APPDATA%\Bareline` are migrated; `%APPDATA%` is also used when `LOCALAPPDATA` is not set.

A damaged, oversized or UTF-16 `settings.toml` does not stop Bareline from starting: the file is set aside or converted, a notice explains what happened, and the editor starts with default settings.

## One window per profile

A second launch with the same profile hands its files to the running Bareline window and exits. `--new-instance` (or the Notepad++ spelling `-multiInst`) opens a separate window instead. Files dropped on the window open as documents; a dropped folder opens as the workspace. **File > Document > Open in New Instance** and **Move to New Instance** move a saved, unmodified document to a separate window.

See [Command line](../../README.md#command-line) for every launch option.
