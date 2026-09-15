# PR-T01 — Proven-owned cache cleanup

- Issue: TECH-001
- Baseline source: `6fdaf57ed55f99c2146d8d647fa41d8721efe608`
- Audit reproduction binary: SHA-256 `e3f283c8cc61ccae5d1549a1136b899d02abf3cf95a27c9fc7be8a42d7128702`
- Dependency: PR-T17 integrated in the private snapshot.

## Scope

Define a reusable `crates/file-io` ownership and bounded cleanup policy for Bareline-created temporary caches, with Windows no-follow/reparse and process-identity operations in `crates/platform-windows`. Replace launch cleanup's wildcard/PID-guess deletion. Preserve all unproven legacy, linked, referenced, live, unknown-liveness, cancelled and failed-deletion candidates with structured reasons. The policy is intended for reuse by PR-T05.

## Producer grammar and decisions

- The exact top-level roots are `Bareline-owned-spill`, `Bareline-transcode`, `Bareline-compare-staging`, `Bareline-power-staging`, and `Bareline-drag-staging`. No `Bareline-*` wildcard remains.
- Owned-spill producers create `resident-spill-<pid>-<serial>`, `spill-baseline-<pid>-<serial>`, `owned-segments-<pid>-<serial>`, and `owned-stream-<pid>-<serial>`. Transcode creates `bareline-transcode-<pid>-<serial>`. Compare, power, and drag staging use only the owned-stream grammar. Root/kind combinations outside this registry are rejected.
- `bareline-tail-<pid>-<serial>.raw` is a file scratch format and is not a directory sweep candidate. Recovery directories live below profile roots and are not temporary-cache roots.
- Call sites can supply custom quota, test, or private-profile cache roots. Writers continue to operate there without publishing a cleanup record; only a registered top-level root opts into this janitor.
- A record is created with `create_new`, synced, and renamed before content is written. It carries format/kind, a random 128-bit directory nonce, PID/process creation time, and retained-recovery references.
- Sweep candidates require a registered root/kind/grammar, a valid record, no recovery reference, and `Dead` liveness. Access/query failure and PID creation-time mismatch are `Unknown` and retained.
- Windows obtains no-follow guards and volume/file identities for root and candidate. The sweep retains both guards across bounded record reading and the liveness decision, releases the candidate guard, and deletion reacquires guards and validates both identities before traversal. Every descendant directory is guarded and every reparse entry is rejected.
- A shared consumable entry/deadline budget covers candidate enumeration, successful deletion, and failed traversal. The platform reports visits even on error, and the sweep stops immediately on cancellation or exhaustion.
- The policy selects the first bounded, structurally valid owner record or retained-cleanup receipt and passes that exact filename and byte sequence into deletion. Windows re-reads and compares the same proof after reacquiring the candidate guard, so it cannot remove or restore authority selected from a different path. Invalid proof files without a valid fallback remain unproven and retained.
- The owner record is removed only at the final candidate-directory operation. After descendant guards retire, Windows opens a no-follow delete-capable handle with no delete sharing and revalidates the candidate identity. It removes the selected proof and applies directory disposition through that pinned handle. A failed disposition restores the bounded proof while the same identity remains pinned, as the owner record or a retained-cleanup receipt. Failure of both restoration paths is surfaced in the structured report and aborts the sweep. Filesystems that cannot apply handle disposition retain the candidate and report deletion failure.
- Recovery does not retain these temporary cache paths. `seal_baseline` copies and syncs `baseline.bin` inside the profile recovery generation; streaming recovery visits the source ranges and copies inverse/inserted bytes into hashed, synced `segment-<revision>.bin` files before journaling them. Replay opens only those generation-local blobs. Production owner records therefore publish an empty retained-recovery list by construction; the field remains versioned for future policy changes, and the sweep still conservatively rejects any nonempty record.

## Changed files

- `crates/platform/src/lib.rs`: reusable cache identity/liveness/deletion capability contract.
- `crates/file-io/src/owned_cache.rs`, `lib.rs`, `owned_store.rs`, and `codecs/disk.rs`: exact producer registry, atomic records, conservative sweep, and producer publication.
- `crates/platform-windows/src/owned_cache.rs`, `files.rs`, and `lib.rs`: process creation/liveness queries, guarded directory identity, bounded no-follow deletion, and Windows fixtures.
- `apps/bareline/src/windows_app/launch.rs` and `windows_app.rs`: replace wildcard startup deletion with the bounded worker sweep and structured completion counts.

## Focused checks

- Worker Cargo runs are prohibited; root will run exact contained commands after review/integration.
- Worker static checks: `rustfmt --edition 2024` on every changed Rust file and `git diff --check`.
- Deterministic policy fixtures cover exact registry/grammar, one dead removal, live/unknown/PID-reuse/unmarked/recovery-reference retention, injected identity change, deletion failure, cancellation, and entry cap.
- Windows guarded-deletion fixtures cover a successful nested purge, a nested junction with an external sentinel, replacement root identity, finalization-time candidate replacement, a locked-entry partial purge, and failed disposition with the primary owner name blocked and retained-receipt fallback.
- Root commands after integration:
  - `cargo test -p bareline-file-io owned_cache::tests --offline --locked`
  - `cargo test -p bareline-platform-windows owned_cache::tests --offline --locked`
  - `cargo test -p bareline --bin bareline windows_app::launch::tests --no-default-features --offline --locked`

## Unresolved qualification

- PR-T02 owns migration correctness/reconciliation; PR-T05 consumes the deletion policy for recovery retirement.
- Final integrated compile, native Windows junction/subprocess evidence, and the full lifecycle suite after shared file-safety PRs remain root-owned.
