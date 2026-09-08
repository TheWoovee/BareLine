# PR020 fault-transition harness

Status: source prepared; execution NOT_STARTED. No process has been killed while implementing these fixtures.

## Coordinator run

Run once in the closure-020 checkout:

- `cargo test --locked -p bareline-file-io --test recovery_process_death`
- `cargo test --locked -p bareline-file-io --lib lifecycle::fault_transitions`

Record commit, OS/filesystem, command, counts and failures in PR-020 evidence. These are process-death and injected-I/O-error tests, not physical power-loss certification.

## Transition coverage

| Production transition | Injection | Restart invariant |
|---|---|---|
| Recovery segments written / flushed | Existing `Boundary`, StorageFull + owned child kill | Prior or next complete record only; durable flushed record survives |
| Journal record written / flushed | Existing `Boundary`, both modes | Recovered text and inverse/inserted history share exactly the validated revision prefix |
| Checkpoint temporary written / replacement committed | Existing `Boundary`, both modes | Baseline and validated journal reconstruct complete text; original is untouched |
| Save stage created / bytes written | Private cfg(test) thread-local seam, both modes | Target remains old; any surviving stage is empty or complete fixture bytes |
| Before / after stage flush | Same | Target remains old; no hybrid target |
| Expected fingerprint checked / immediately before replacement | Same | Target remains old |
| Replacement returned / target fingerprint checked / receipt return | Same | Target is complete new bytes with matching SHA256; retained fixture backup remains old |

The save fixture exercises the production `save_bytes` state machine with a portable rename adapter. It does not substitute copy-in-place for replacement. Its backup is a separately retained fixture artifact, not a claim that Save creates a product backup. Recovery fixtures validate both edit directions and monotonically matching receipts through read-only replay; they do not claim to restore an editor UI undo stack.

Each parent creates a unique private scratch directory, spawns the current test executable with one exact child test, waits at most 30 seconds for that child's reached marker, then kills and reaps only its owned `Child`. A drop guard kills/reaps on assertion or timeout. No global process enumeration, process-name killing, shell child, user data, or GUI automation is used. Child stdout is suppressed; errors carry transition names, not document contents. Normal direct execution of a child test does nothing without the dedicated environment marker.

## Remaining distinction

Process death may preserve unflushed kernel cache bytes. Therefore pre-flush recovery accepts either complete old or complete new validated prefix; it never requires unflushed data to disappear. Actual device power failure and filesystem guarantees inside an OS replacement syscall require platform/storage testing beyond a process kill. Native syscall-boundary fixtures are prepared below; no executed native coverage result is claimed here.

## Native replacement boundary coverage

Prepared Windows-only command: `cargo test --locked -p bareline-platform-windows --lib files::replacement_faults`.

Four owned-child fixtures cover before/after each real `ReplaceFileW` (existing target) and `MoveFileExW` (new target). The production result is captured before the post-call test hook. Restart verifies exact expected target bytes and SHA256, unchanged retained backup, and expected stage consumption. These hooks compile out outside tests and do not inject inside an indivisible OS syscall. Execution remains NOT_STARTED.
