# PR-T17 — Keep informational and isolated launch modes free of unrelated mutations

Priority P2. Fixes TECH-020. Dependencies: none. Land before PR-T01/T02 because all touch `apps/bareline/src/windows_app/launch.rs`; those PRs then own safe cleanup/migration internals. Also inspect `apps/bareline/src/main.rs` and the launch caller's mode selection.

## Reproduction

`../evidence/cli-side-effects.json` records the actual rebuilt binary run with `--version` in a generated private profile. It printed the expected version with exit 0, but moved `roaming/Bareline/settings.toml` into LOCALAPPDATA and deleted a TEMP cache sentinel. `parse` invokes `migrate_from_roaming` at line 350 and `sweep_temp_caches` at line 404 before returning help/version flags. It also computes installed-profile migration before selecting a portable or performance-specific root.

## Implementation

1. Split argument interpretation, launch-mode selection, path/config discovery and persistent initialization. Parsing must not migrate, create, delete or scan profile/cache trees. Return a typed mode (`Help`, `Version`, `Diagnostic`, `Portable`, `Installed`, `Performance`) and validated raw options.
2. Handle Help/Version immediately after argument validation and before profile preparation. Print stable output and exit with no window, session, recovery, extension or cleanup work. Invalid arguments must also avoid persistent mutation.
3. Select the authoritative root before initializing anything. Portable mode uses only its portable root; performance/native diagnostic fixtures use their isolated roots and declared instrumentation. Installed profile migration must never run as a side effect of these modes.
4. Schedule ordinary installed migration/cleanup only after first frame and only for the chosen profile, using PR-T01/T02's safe/resumable implementations. Separate process executable/DLL search hardening from profile mutation, preserving security hardening without changing user data.
5. Keep CLI relative-path resolution against the original launch CWD before hardening changes CWD. Preserve all supported options and single-instance handoff behavior.

## Verification

Subprocess tests with fake APPDATA/LOCALAPPDATA/TEMP and sentinel files must run `--version`, `--help`, invalid options, portable launch, and diagnostic/performance launch. Hash/tree snapshots must remain unchanged outside each mode's explicitly owned outputs. Installed launch must still schedule the intended migration and safe cleanup. Verify neither informational mode initializes a native window or invokes network/extension work. Cover paths beginning with options after `--`, non-ASCII paths and contradictory flags.

Acceptance: exact version/help output with no profile/cache mutations, portable launch leaves installed data untouched, normal launch retains its intended functionality, and no first-frame filesystem sweep was merely moved into another synchronous constructor.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
