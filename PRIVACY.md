# Privacy policy

Bareline is a local text editor. It has no account, analytics, advertising, telemetry, or crash-report upload, and it does not send your documents anywhere. This policy covers the Bareline editor, its update helper and its Windows installer.

## Data kept on your computer

Bareline stores the following in its [profile folder](README.md#settings-and-local-data) (`%LOCALAPPDATA%\Bareline`, or `data\` next to a portable copy):

- **Settings and shortcuts:** `settings.toml` and `keymap.toml`.
- **Session and recovery:** `session.json` and recovery journals. They contain paths of open files and can contain unsaved document text, so Bareline can restore your work after a restart or crash. Turn off **Restore previous session** in Settings to skip session restore.
- **Recent files:** the File > Recent Files list, stored as file paths.
- **Macros** you record, and extension data when extensions are enabled.
- **Diagnostic logs:** `bareline.log`, `bareline.previous.log` (each at most 512 KB) and a single `bareline.crash.log`. They hold version, build, renderer, timing and memory figures and, after a crash, the source-code location of the failure. By design they never record document text or file paths.

Uninstalling keeps the profile so your settings and recovery data survive a reinstall. Delete the profile folder to remove this data.

## Windows Recent items and Jump List

When Bareline is installed (not portable), it adds the path of each file you open, including files you pick in its Open and Save dialogs, to Windows Recent items, which Windows keeps under your Windows user account. Windows can then show the file in File Explorer's Recent list and Quick access and, for file types Bareline is registered to open, in Bareline's taskbar Jump List.

To stop this, open Settings and turn off **Add opened files to Windows Recent items** under Files (`add_to_windows_recent = false` in the `[files]` table of `settings.toml`). Only your user settings can change it, not a workspace folder. Turning it off stops Bareline and its Open and Save dialogs from adding new entries; it does not remove entries Windows already has. To remove those, remove them from the Jump List or clear recent items in File Explorer or in Windows Settings (Personalization > Start). A portable copy never adds files to Windows Recent items. Independently of this setting, File Explorer itself can record a file you open by double-clicking it there; Bareline does not control that.

## Diagnostics you choose to share

**Help > About Bareline > Copy diagnostics** copies the version, build commit, project links, architecture, renderer, profile mode and whether local logging is on to the clipboard. It contains no file paths or document text. Nothing is sent unless you paste it somewhere, for example into a bug report. Recovery files, sessions and logs are shared only if you attach them yourself.

## Network use

- **Editing:** opening, editing, searching and saving files does not use the network. Files in network, synchronized or cloud folders are handled by that storage service, under its own terms.
- **Updates:** preview builds have no updater network access. A configured release with updates enabled contacts only the update host named in its release configuration, over HTTPS, and only when you run the update check or confirm an update. The requests download the signed update metadata and the update itself; they send no document data, account or device identifier, cookies or credentials, and identify themselves as `Bareline-Updater/1`. The update host, like any web server, can see your IP address and the time of the request.
- **Extensions:** extension downloads and execution are disabled in the default build.
- **Tools you run:** external commands run only after you approve them and can use the network themselves. Remote-file operations require your permission.

Windows, printers, storage services and external programs follow their own privacy terms; this policy does not claim that they never use the network.

## Downloads and project services

Releases, issues, pull requests and private vulnerability reports are hosted by GitHub under [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement). Code signing, if it is approved later, is a release-build step and does not receive documents from the running editor ([code signing policy](CODE_SIGNING.md)).

## Changes and contact

Changes to this policy are made in this file and are visible in the repository history. Ask questions in [GitHub issues](https://github.com/TheWoovee/BareLine/issues), or use [private vulnerability reporting](SECURITY.md) for security concerns.
