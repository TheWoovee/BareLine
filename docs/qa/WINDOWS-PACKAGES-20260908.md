# Windows package verification — 2026-09-08

Portable smoke test: PASS.

Installer compile: PASS using the exact compiler-version probe for Inno Setup 6.4.3. Artifact `dist/windows/0.1.0-local-20260908/bareline-0.1.0-windows-x64-setup.exe`, 4,794,789 bytes, SHA256 `d927d08e655cdf49124e4c6b5c121cac82d53222a664c52c461a2287bc2af5c0`. Installer and payload executables are unsigned local builds; no install/upgrade/uninstall or signing/publication is claimed. Installer defaults to per-user installation with optional machine scope and unchecked shell registrations.

Build: one optimized release invocation for bareline and bareline-update-helper, without QA features; completed successfully with three dead-code warnings. Logs are in target/package-20260908. Packaging script tests passed; all six ZIP entry hashes matched the staged payload. CycloneDX output contains resolved dependency references and no developer-local paths. The optional extension host is not included. Artifacts represent the current local working tree, including uncommitted QA fixes.

Artifact: `dist/windows/0.1.0-local-20260908/bareline-0.1.0-windows-x64-portable.zip`.

- ZIP SHA256 verified: `0543aa466b79c1a0e4504a093895a98492c3559ca3c73066f53b4e317cedb5ab`.
- Extracted bareline.exe SHA256 verified: `4c62131996d147a83cd2a6f9026eeaf62a17896cea3d3a923758575f0a0df05c`.
- Extracted into fresh `tests/e2e/results/package-20260908-portable`; package contains editor, update helper, portable marker, license, SBOM and third-party notices.
- Sky launched extracted executable directly. About reports version 0.1.0, x86_64, Hardware renderer and **Mode: Portable**.
- Typed generated `Portable café 🙂`, saved as extraction-local portable-qa.txt. Exact UTF-8 bytes match independently constructed expected content (19 bytes).
- Changed appearance from System to Light through Settings. Local `data/settings.toml` records theme mode light.
- Clean Alt+F4 exit verified by exact-path window enumeration returning no windows. Local `data/session.json` records portable-qa.txt, caret/anchor 19, active tab 1, compare null.
- Relaunched the same extracted executable. Light theme, clean saved document, correct Unicode text, 19 bytes, and caret line 1 column 16 restored.

All native interactions used Sky. Screenshots and accessibility snapshots are retained in task tool history. No installer installation was performed by this smoke test; installer compilation/static verification is separate. Existing QA instances and personal apps were preserved.
