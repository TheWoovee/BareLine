# Windows lab qualification handoff

The three driver paths are implemented in `native_lab.ps1` and `native_lab_driver.ps1`, with headless contract checks. **They have not been executed on a real VM.** Without explicit `--lab-config` inputs the adapter returns NOT_RUN. Authored procedures and fixture identities are not production acceptance.

## Driver inputs

Pass `--lab-config C:/lab/crash.json` before the runner-supplied request argument in the adapter argv. All configurations contain `schema_version: 1`, the exact `journey`, actual `machine_uuid`, `snapshot_id`, and an `assets` array. Each asset has a unique `id`, absolute `path`, safe relative destination `relative`, and exact `sha256`. Assets are copied into fresh scratch under quotas and rehashed. The driver verifies the machine UUID/model and unelevated token before launching an asset. Snapshot identity is an operator attestation, not an automated snapshot observation.

- **crash_recovery:** require asset `recovery_probe` and `save_point` equal to `StageFlushed`, `BeforeReplace` or `AfterReplace`. Build the diagnostic editor with `cargo build -p bareline --features qa-faults`; build the probe with `cargo build -p bareline-file-io --example recovery_inspect`. The explicit owned target/nonce/PID boundary signal pauses at actual save code and aborts after 30 seconds if unreleased. Shipping configured builds reject this capability, including dependency feature unification. These diagnostic results cannot qualify signed shipping identity. The probe copies bounded small checkpoint fixtures without writing to their original journals.
- **extension_isolation:** require assets `runtime` (the host executable selected by Install runtime), `catalog`, and every detached metadata/signature/component file in its exact relative package layout; also require `host_sha256` and three `packages` entries with `kind` (`json`, `xml`, `hex`), `catalog_index` (0–2) and exact `installed_label` from the reviewed catalogue. The product performs real signature and permission verification. If the host finishes before the crash observation, retain the failed attempt; do not infer a crash.
- **install_update_rollback:** require asset `installer`, `publisher_sha256` (certificate DER digest), `helper_sha256`, `update_sha256` and a nonzero `activation_failure_exit_code`. The candidate's compiled endpoint must serve the reviewed signed deliberately failing activation fixture. The driver disables optional installer tasks, uses actual Check/Apply on Exit, observes failed activation and retained rollback bytes, runs the authenticated `--recover` helper, verifies the restored editor, and invokes the real uninstaller. It does not provision hosting or sign assets.

The three paths retain step observations and owned cleanup. Broader fault boundaries, optional association variants and independent qualification below remain required. A missing asset, unsupported observation or failed prerequisite cannot pass.

## Shared setup

Create disposable standard-user Windows 10 build 19045 and Windows 11 build 22631+ machines with snapshots. Pin OS/toolchain, candidate/source, config, complete artifact hashes and environment cells. Use generated files only. Record before/after installed files, owned PIDs/creation times, registry association options, tasks/services and isolated profile. Retain failed/stopped attempts. Use `typed_producers.py` manual observations for a genuinely observed independent lab procedure; complete reviewed mappings are still required.

## Crash/recovery

1. Open one saved fixture and one Untitled document, record exact edits/revisions and recovery durability acknowledgements; capture journal and baseline identities.
2. Exercise the existing production save/fault contracts on an owned disposable disk: terminate only the owned editor at declared staged-write, flush, replacement and recovery-retirement boundaries. A timer alone does not prove the boundary. Add a test-only synchronization probe if the boundary cannot be observed; never add a production bypass.
3. Restart the same isolated profile once. Verify original-or-complete-replacement bytes, acknowledged recovered edits, opaque-byte provenance and honest Complete/Edits-only status. Check cancelled close retains recovery, accepted discard stays discarded, and a busy single Close/Exit resumes exactly once. Preserve minimized failing fixtures.

## Extension install/crash/revoke

1. Supply reviewed authority/catalog/runtime/component metadata with real signatures and configured trust. Capture absent-runtime/disabled state; install explicitly, verify all bytes and grant decisions. Include wrong signature/hash, expiry, rollback and revoked-key negative controls.
2. Invoke JSON, XML and Hex with exact input/expected output and one Undo for edits. Bind each dynamic command to the composed runtime inventory. Keep the document responsive and snapshot/revision/grant identities fixed.
3. Terminate only the owned extension host during a staged operation, observe editor survival and discard of partial edits, then revoke/disable the package and verify queued/running/cached invocation cannot execute. Check host/process cleanup and offline restart/restore. Existing host Fast tests are useful controls; they do not exercise this installed editor journey.

## Install/update/rollback/uninstall

1. Verify the final signed installer and inner/external assets independently. Install as standard user with default options, then the declared optional association choices in separate VM snapshots. Compare exact installed files and authority against the signed inventory. UserChoice, services and scheduled tasks must not be created implicitly.
2. Feed valid and rejected signed updates through the actual editor/helper path. Inject activation failure/interruption at observed stage boundaries. Verify monotonic metadata/root floors, original-or-complete-new executable, durable journal and retained rollback bytes. Rehearse root rotation/revocation and the separately authenticated sole-root recovery procedure.
3. Recover/rollback using the supported helper, then uninstall. Confirm correct version starts, user documents/profile are preserved according to chosen scope, and optional associations are removed without changing unrelated registrations. Repeat for portable relocation and offline startup using the actual final ZIP; the local generated portable fixture does not qualify a signed ZIP.

Execution and qualification remain open until those observations exist. Signed assets and VM snapshots must be passed explicitly; do not guess paths, run an installer on the owner's daily profile, or convert a missing prerequisite into PASS.
