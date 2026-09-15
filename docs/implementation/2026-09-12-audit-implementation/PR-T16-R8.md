# PR-T16 R8 — Paged save terminal publication

## Base

- Reviewed local snapshot: `ab3e4ce1adb7c93d31810c1bc07a2f07c6549d65`
- Branch: `codex/audit-20260912-t16-paged-save-terminal-r8`

## Native evidence and diagnosis

The R6 native fixture committed the exact edited bytes and the paged file service published its
saved content state: the on-disk length and hash changed and the tab's dirty marker cleared. The
workspace nevertheless continued to show `Saving…` because the paged save submission is the only
save path that sets that transient message without owning or consuming a corresponding terminal
receipt. `Workspace::pump` pumps the paged surface, conflicts, and cleanup warnings, but it never
clears the paged save message after the surface receives the worker completion.

The concurrently visible `Line numbers estimated · indexing` state has a separate producer.
`GlobalNavigation` runs its scan on `bareline-global-line` and publishes a generation-qualified
nonblocking `IndexReceipt`; the surface reads that receipt without polling the worker index lock.
Its continued CPU use can explain the long indexing interval, but it cannot resolve the save
message and is not evidence that the already committed save lacked a terminal receipt.

## Intended correction

Carry the typed paged lifecycle outcome through the existing surface worker completion and expose
it once to the workspace. The workspace will resolve only the matching `Saving…` presentation
state when that terminal is consumed, while preserving typed conflicts, encoding failures, and
cleanup warnings as the authoritative outcomes. Add a controlled small-fixture regression that
observes submission, committed bytes, terminal pumping, and removal of the transient message.

## Implemented boundary

- Each admitted surface save receives a document-qualified, monotonically generated
  `PagedSaveOwner`, shared across peer views of the same actor.
- The surface publishes the actual `PagedLifecycleReceipt` to its presentation queue immediately
  after `PagedSession::execute(Save)` returns. Recovery retirement, actor reacquisition, viewport
  refresh, and the general worker completion continue afterwards, so `busy()` still protects close
  and recovery ownership.
- The workspace tracks admitted owners, consumes only an exact matching document/generation
  terminal, and clears only the still-current `Saving…` text. A later unrelated message is left
  intact. Typed errors replace the transient message; completions which fail before producing a
  file-layer receipt are retired when the surface worker reaches its terminal state.
- The regression uses a five-byte forced-paged source, makes one exact-byte edit, and holds the
  recovery retirement commit. It verifies the saved six bytes and terminal banner while the
  surface remains busy, then verifies a later message survives and releases maintenance.

## Validation ownership

The implementation worktree receives source edits, formatting, and diff inspection only. The root
agent owns Cargo compilation and tests in the serialized original checkout.

Performed here: `rustfmt --edition 2024` on the two edited Rust files and `git diff --check`.
Cargo/build/native execution was intentionally not run.

## R1 ownership tightening

- Only the tracked workspace entry points allocate an owner and publish into the receipt mailbox.
  Compatibility `save_prepared` and `save_copy_prepared` calls retain their prior completion model
  and cannot accumulate orphaned receipts.
- Banner resolution also requires that no resident save ticket remains pending. A paged terminal
  therefore cannot clear the identical `Saving…` text owned by a later resident save.
- Workspace tracking is intersected with the active owners still held by live paged surfaces after
  every pump, retiring admissions which failed before producing a file-layer receipt or whose view
  was retired.
- The maintenance-hold check now installs the newer message before consuming the paged receipt.
  A second controlled test blocks a resident commit, completes a paged save, and verifies the
  resident banner remains until its own ticket completes.
