# PR-T05 R7: Close-command and TaskDialog diagnostics

- **Baseline:** `0751cfa082217ecc16d4d11bb18f43a066593398`
- **Native evidence:** Clean File > Exit, Run > Run, and File > Output > Save Copy all dispatch successfully from the native menu in a durable redirected process. A dirty File > Close/Exit still produced no prompt in the auditor's main-window capture. Save Copy to an existing target became modal until Escape and then reported cancellation, which shows that the current main-window/window-enumeration oracle can miss an owned TaskDialog. Native menu ingress is therefore not identified as the cause.
- **Source finding:** Prompt failures from `TaskDialogIndirect` were converted to `SaveChoice::Cancel`, making API failure indistinguishable from a real Cancel. The queued command record also discarded its source HWND before app dispatch, so diagnostics could not distinguish a command for the main window from a thread-queue collision.

## Change

- Preserve the queued `WM_COMMAND` HWND and reject control notifications (`lParam != 0`) and other-window messages before command lookup. Existing message-hook dispatch remains unchanged; no menu subclass or private-message bridge is added.
- Preserve the raw TaskDialog selected ID and keep a failed HRESULT distinct from Cancel. A failure leaves dirty data open and publishes a scoped status.
- When `BARELINE_QA_COMMAND_TRACE` names a unique absolute file, create its parent and truncate/create the file at process start. Flush at most 24 content-free JSONL records. Records cover raw HWND/ID receipt, owner acceptance or rejection, stable command/action resolution, close queued/deferred/ready transitions, TaskDialog entry, exact selected ID (`1101`, `1102`, or common Cancel `2`), and HRESULT failure. Transition records are deduplicated by request ticket and stage. Tests use an explicitly disabled trace, so they cannot truncate an audit file through inherited environment.
- The dialog-entry record is flushed immediately before the synchronous `TaskDialogIndirect` call. A trace ending there identifies an in-progress prompt that the harness did not observe; a return or failure record distinguishes the native outcome.

## Focused verification for integration

Root-owned commands:

```text
cargo test -p bareline-platform-windows command_ingress_preserves_owner_and_rejects_controls --offline --locked
cargo test -p bareline --bin bareline deferred_close_tests --offline --locked
```

Static checks in this worktree: scoped rustfmt, `git diff --check`, constructor/caller census for `Shell`, `CloseTarget`, `confirm_save_document`, and `confirm_save_all`.

## Remaining native qualification

Launch one fresh durable process with a unique `BARELINE_QA_COMMAND_TRACE` path, make one dirty edit, invoke one dynamically resolved native Close or Exit command, and preserve the JSONL trace before any Escape. The prompt observer should separately capture foreground/owner HWNDs, main-window enabled state, invisible same-PID top-level windows, descendant class/control ID/text, and DirectUI/UIA controls. Production close behavior must not be changed solely to satisfy a main-window-only capture oracle.

The unrelated `std::io::stdio.rs:1166` panic occurred in a process whose inherited output pipe had closed; the same Run command passed in a durably redirected process. This slice does not change process execution or general stdout/stderr policy.
