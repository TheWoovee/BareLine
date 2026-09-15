# PR-T09 — Fix flaky synchronization checks and preserve trustworthy run results

Priority P2. Fixes TECH-011/012. Dependencies: none. Scope: `crates/editor-surface/src/paged_view.rs::ActorGate` tests; `xtask/src/journey.rs`; test-result recording. This PR must not silently change close/discard behavior without a proven defect.

## Fresh failures

Workspace Cargo test exited 101: `actor_gate_wait_returns_within_the_timeout_when_the_worker_is_idle` asserted elapsed >= 20 ms but saw 19.9607 ms. Ten isolated reruns passed. The method calls `Condvar::wait_timeout` once and does not promise a minimum blocking duration. A spurious/early return is not proof of broken actor synchronization.

Sol's first native `journey all` was 12/13 because p0-7 did not find a save prompt; three isolated reruns passed. A subsequent aggregate against the rebuilt current executable was **11/13**: p0-7 failed again, and p4-4 detected no increase in colorful emoji pixels (saturation 165 before and after). Keep [the current raw log](../ui-ux/evidence/journey-current-binary.log) distinct from the initial aggregate and isolated reruns. It is not yet known whether focus/input delivery, a clean document, prior state, capture geometry, or product behavior caused these failures. Earlier native observations retained emoji text; they do not resolve the color-rendering assertion.

## Implementation

1. Replace the strict lower-bound assertion with tests of the actual contract: waiter eventually progresses, release notification is observed, callers retry actor acquisition after spurious wake, cancellation exits, and idle wait cannot deadlock. Use barriers/channels and an outer generous watchdog; do not merely increase a sleep until green.
2. Audit `ActorGate` callers to confirm they recheck state in a loop. If a real lost-wakeup problem is found, implement a predicate/generation counter with the mutex and test both notification-before-wait and notification-during-wait. Do not require production Condvars to sleep for a minimum duration.
3. For p0-7, record exact binary/source hash, isolated roots, focused HWND, active document identity, dirty state/UIA content, command injection path and dialog tree before each step. Wait for observable editing/dirty state before close; do not use a timing delay as proof that input was received.
4. Retain failed aggregate evidence even when retries pass. A retry must be reported as retry, not replace the initial failure. Distinguish harness setup failure, product assertion failure, timeout and blocked environment.
5. Count top-level tests separately from child fixture invocations. The normal workspace log contains subprocess test summaries that otherwise inflate pass totals. Bind runs to the dirty working-tree/source manifest as well as HEAD.
6. For p4-4, retain the before/after screenshots, target HWND/client bounds, DPI/font/renderer settings, document text/caret state, typed-input receipt and completed-frame receipt. Confirm the intended emoji reached the editor and the detector sampled its actual glyph region. Check color-glyph rendering separately from Unicode text retention. Correct a defective detector only with evidence; if current product rendering fails, create a narrowly scoped renderer/font fix with that reproduction rather than weakening the assertion or accepting every screenshot.

## Acceptance

Run the actor tests under normal and loaded scheduling and inject a spurious notify. Run p0-7 and p4-4 alone and in the entire native suite from fresh private profiles after adding diagnostics. Capture enough state to explain any failure before closing it. A passing rerun alone does not satisfy the PR. The documented test totals and initial failures must match raw logs exactly. Native app/UIA/IME qualification remains separate from harness tests.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
