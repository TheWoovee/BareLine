# Remaining session restoration correction

Scope: ISSUE-030 caret restoration follow-up in `docs/qa/RETEST-NATIVE-20260908.md`. The original launch failure did not reproduce; the follow-up saved active tab 11/caret 69 but reported line 1/column 1 after restart.

Contract: PR-004 session persistence requires independent caret/anchor, scroll, pane membership and active-tab restoration. ADR-33 retains first-frame-gated incremental loading.

Source correction: install the persisted view controller after rebuilding the comparison. Comparison setup calls `compare_pair`, which chooses panes, assigns a document and reinstalls views; running it after session views can replace saved selection and active-pane state. The persisted controller now supplies the final tab membership, caret/anchor, scroll and active tab after comparison initialization. Comparison computation still retains its source snapshots and options.

Validation: static call-path review only. No test, build or native/UI run was performed in this change. Native acceptance remains pending: exit a saved split/compare session with distinct primary/secondary carets and secondary active, restart, and verify both selections, active tab/pane, scroll and comparison source identity. The historic startup failure is not claimed fixed by this correction.
