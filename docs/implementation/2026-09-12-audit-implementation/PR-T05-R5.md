# PR-T05 R5: Retain busy close intent

- **Baseline:** `861522334c777282bbce79cb8aa66048a3ce980c`
- **Scope:** Preserve one document-close or application-exit request while recovery, settings, or I/O is busy, then resume it from the existing shell wake/pump path without UI-thread waiting.
- **Safety:** Stable document/tab/revision ownership and full application dirty-set revalidation remain authoritative. Explicit user cancellation and terminal save failures do not requeue themselves.

## Evidence

- `rustfmt --edition 2024 apps/bareline/src/windows_app.rs` — passed.
- `git diff --check` — passed (Git reports the repository's expected LF-to-CRLF warning only).
- Literal census: all `CloseTarget` constructors initialize the new deferred state; all three application terminal variants pass through the retained busy boundary before prompting or exit.
- Added deterministic headless regressions for a gated application recovery scan, a gated document save with tab reordering, explicit Cancel retirement, and revision change retirement.
- Repeated busy dispatch is asserted to retain exactly one request without a self-wake or native pending-operation dialog. Test gates use one predicate mutex, bounded waits, and an unwind release guard.

Root-owned contained command proposed after integration:

```text
cargo test --offline --locked -p bareline --bin bareline deferred_close_tests
```

Native one-shot Close/Exit qualification remains pending after integration.
