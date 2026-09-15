# PR-T18: Launch request lifecycle

- **Baseline:** `fd428fd7fe056d5cfb3eff1d9d62a6a60d535329`
- **Scope:** Give accepted launch paths stable request ownership, consume terminal open/navigation outcomes, bound outstanding work, and prevent failed, cancelled, closed, or stale requests from activating documents later.
- **Coordination:** Keep profile migration and unrelated launch initialization unchanged.

## Evidence

- `LaunchRuntime` owns at most 256 path tickets. Each ticket advances through `Queued`, `Opening`, `Opened`, `Navigating`, `Monitoring`, and a terminal state; terminal requests are removed instead of becoming lifetime capacity debt.
- Workspace emits one `LaunchOpenOutcome` for every tracked resident/paged success, admission failure, worker disconnect, decode failure, or open failure. `launch_request: None` was added to every non-launch `PendingIo` constructor; tracked open and streaming continuation are the only `Some(id)` producers.
- Paged navigation captures the read handle's full source identity and applies its offset only when that full identity is unchanged. Closing the owner cancels its task. Poll loops, including the inner viewport-window loop, observe the T06 cancellation token.
- Activation occurs once when an open receipt becomes actionable. Waiting for a complete resident snapshot or the single paged navigation slot does not repeatedly restore that tab after the user changes selection.
- IPC assigns tickets before acknowledgment, cancels those tickets if the sender has disconnected, and withholds acknowledgment when workspace/admission fails. `PendingOpen::accept` now reports whether the acknowledgment was delivered.
- Monitor admission retries only queue saturation and retires after three attempts; permanent failures are terminal.
- Launch monitor startup resolves the request's document in the primary workspace and bypasses the active-pane command routing, so an active secondary pane cannot redirect monitoring to another file.
- Navigation tasks are processed before open requests waiting for the single navigation slot. A later-listed completed navigation releases the slot before an earlier waiter is visited, without relying on another wake.
- Deterministic regressions cover 300 sequential failed receipts, the 256 concurrent bound and flag preservation, reversed waiter/completion order, navigation-owner cancellation with subsequent worker progress, a forced-paged line lookup after viewport movement, document-bound monitoring with a different secondary identity, tracked missing-file terminal receipts followed by a valid open, and abandoned IPC acknowledgment.

Static checks run in the worker checkout:

- `rustfmt --edition 2024 --check` for all five changed Rust sources: pass.
- `git diff --check`: pass.
- Caller/constructor census: `LaunchRuntime::queue` has startup, IPC, and focused-test callers; `PendingOpen::accept` has the IPC consumer and two platform tests. A literal recursive census across `crates/app/src` finds eleven `PendingIo` constructors: two open-pipeline constructors propagate the optional launch ID and all nine non-launch constructors explicitly set `launch_request: None`.

Requested contained integration commands:

- `cargo test --offline --locked -p bareline-app workspace::tests::tracked_missing_opens_emit_terminal_receipts_without_accumulating -- --exact --test-threads=1`
- `cargo test --offline --locked -p bareline windows_app::launch::request_tests -- --test-threads=1`
- `cargo test --offline --locked -p bareline windows_app::watch::tests::launch_follow_resolves_its_workspace_document_when_secondary_is_active -- --exact --test-threads=1`
- `cargo test --offline --locked -p bareline-platform-windows instance::tests::abandoned_sender_cannot_be_reported_as_accepted -- --exact --test-threads=1`

## Deferred qualification

- Contained Cargo regressions and native missing-file-to-valid-file handoff plus gated large-file close/reopen acceptance are owned by integration.
- Integration must preserve T02's `profile_initialization.settled()` first-frame gate while replacing the adjacent direct `startup_paths` open loop with the launch runtime handoff.
