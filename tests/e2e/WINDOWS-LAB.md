# Windows lab drivers

Crash/recovery, extension isolation, and install/update/rollback run through
`native_lab.ps1` and `native_lab_driver.ps1`. Use disposable Windows virtual
machines with snapshots and generated files. These drivers can terminate the
owned editor or host, install packages, and uninstall them.

Pass `--lab-config C:/lab/crash.json` before the runner-supplied request argument
in the adapter arguments. Without this input, the adapter reports `NOT_RUN`.
Configurations require `schema_version: 1`, the exact `journey`, the actual
`machine_uuid`, a `snapshot_id`, and an `assets` array. Every asset has a unique
`id`, an absolute `path`, a safe relative destination `relative`, and its exact
`sha256`. Assets are copied into fresh scratch under quotas and rehashed. The
driver checks the machine UUID/model and unelevated token before launching an
asset. The operator supplies the snapshot identity.

- **crash_recovery:** supply the `recovery_probe` asset and `save_point` set to
  `StageFlushed`, `BeforeReplace`, or `AfterReplace`. Build the diagnostic editor
  with `cargo build -p bareline --features qa-faults` and the probe with
  `cargo build -p bareline --example recovery_inspect`. The owned target, nonce,
  and PID signal pauses at the actual save boundary and aborts after 30 seconds
  if unreleased. Configured shipping builds reject this diagnostic capability.
- **extension_isolation:** supply `runtime`, `catalog`, and every detached
  metadata, signature, and component file in its exact relative package layout.
  Supply `host_sha256` and three `packages` entries with `kind` (`json`, `xml`,
  `hex`), `catalog_index` (0–2), and the exact `installed_label`. The editor
  performs its normal signature and permission checks.
- **install_update_rollback:** supply `installer`, `authenticode_subject` and
  `authenticode_issuers` (the release configuration's signer pin: subject and
  issuing-CA simple display names; the code-signing EKU is also required),
  `helper_sha256`, `update_sha256`, and a nonzero
  `activation_failure_exit_code`. The candidate's compiled endpoint must serve
  the signed failing-activation fixture. The driver observes rollback, invokes
  the authenticated `--recover` helper, verifies the restored editor, and runs
  the uninstaller. Hosting and signed assets must already be available.

Use only assets prepared for the disposable machine. A missing asset, failed
prerequisite, unsupported observation, or incomplete cleanup fails the run.
