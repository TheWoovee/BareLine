# Windows installer and installed-app smoke — 2026-09-16

## Outcome

Built an unsigned Windows x64 preview installer and portable ZIP, installed Bareline 0.1.0 per user, verified all five installed payload files against their package hashes, and checked the Start Menu shortcut and uninstall registration. The four sequential native journeys passed **12/12 steps**, each ending with clean File > Exit. No product source changes or full test suite were needed.

- Installer: `dist/windows/0.1.0-local-20260916/bareline-0.1.0-windows-x64-setup.exe` (5,151,296 bytes).
- Installer SHA-256: `27bb132433167f2d4f508e2820b174b7b8ac5ea4be94b7a73c0169cd449880d2`.
- Portable ZIP SHA-256: `7c485dd99fcd22d0004fa4ee10ec6f88f7cebf429b51c6c00029ae1f0caf24ed`.
- Installed executable: `C:\Users\Woovee\AppData\Local\Programs\Bareline\bareline.exe`.
- Editor SHA-256: `bc4d5eaca0f12643a6b55bac4785b7b38d32dbfd12333589fe8d732f45e19240`.
- Start Menu: `Bareline > Bareline`; Installed Apps registration: `Bareline version 0.1.0`.

The retained existing Inno Setup 6.4.3 compiler passed the pinned probe. The separately installed newer compiler failed that probe and was not used. Existing cargo-cyclonedx 0.5.9 generated fresh Windows metadata without changing Cargo.lock. Packaging includes actual locked dependency notices, licenses and the normalized merged editor/helper SBOM. No new compiler installation, signing or publication occurred. Optional Explorer/association tasks were deselected; the installer neither requested a reboot nor closed running applications.

## Execution

| Receipt | Observed outcome |
|---|---|
| 01-release-build | Current editor/helper release build passed with `--locked --no-default-features`, 288.9 seconds. No QA-fault or configured-release features. |
| 02-package | Current notices/SBOM and pinned installer compilation passed, 11.3 seconds. |
| 03-install | Actual installation succeeded with exit code zero. The subsequent wrapper assertion failed because Inno registers `Bareline version 0.1.0`, while this temporary checker expected `Bareline`. Preserve this failure. |
| 04-verify-installed | Corrected read-only verification passed against the already installed files, versioned display name, shortcut and deselected optional tasks. No reinstall or product change. |
| 05-column | 3/3: display-column selection across tabs/wide glyphs/short lines, column insertion and exact saved bytes, one Undo restoring all original rows. |
| 06-plain-text | 3/3: emoji/combining/CJK Unicode and CRLF, UTF-8 Save As/Close/Open with exact bytes and clean state, edit/Undo/Redo without changing unsaved disk bytes. |
| 07-code-config | 3/3: actual Rust token pixel colors, indentation and document-word completion with Undo/Redo, exact Save/Close/Open bytes. Owned-window screenshot retained and visually inspected. |
| 08-regex | 3/3: actual multiline PCRE2 query and two matches, capture-expanded preview/Apply/Save, one Undo and Save restoring exact original bytes. |

All captures retained stable source identity during their own run. The build used dirty source at HEAD `e6c1486ff0bb997d96801331b32986d612728d4f`; each receipt retains the actual source manifest. Later progress-document changes do not relabel the tested source. Native runs pin the installed binary before and after; raw results and generated fixtures retain hashes. The four native captures took about 66 seconds combined.

## Scope and remaining work

Host: Windows x64 build **26200.9457**, Ryzen 5 3600, 128 GiB RAM. Native cell: **keyboard, dark theme, software renderer, 100% scale / 96 DPI**. Every run used a fresh generated scratch profile and documents. These are focused checks, not complete acceptance mappings or independent review. No foreground/lock interruption occurred during this smoke subset.

This closes the pending column insertion/save/one-Undo observation in this cell. Six ordinary procedures, three explicit VM lab procedures, required environment variants, signed/configured release assets and broader release qualification remain open. The local preview install does not establish clean-VM update/rollback or signed release readiness. Top-level readiness counts remain **49 = 7 completed/dispositioned + 41 active + 1 deferred**.

## Retention

The [collection plan](collection-plan.json) retains captures, raw logs, native observations/fixtures/screenshots, packaging scripts and installed-payload inventories. [Bundle pointer](retained/bundle.json) and [verification](verification.json) establish byte integrity only; semantic acceptance remains false. Distribution binaries remain in the local `dist/windows/0.1.0-local-20260916` directory and are pinned by the retained checksum inventory.
