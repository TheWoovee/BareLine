# PR-U08 R7 — native split focus diagnosis

Reviewed source snapshot: `64e27f10300a798616dbf388b5a05e0eac1ff209`. Native reproduction used the R6 app identified by SHA-256 prefix `ee4e` and retained evidence under `target/qualification/final-native-audit/evidence/u08-r6-*` in the original qualification checkout.

## Observation

The native editor model demonstrably changed active panes: a pointer click inside Pane 2 moved its visible caret, typing advanced that pane's distinct caret, and F6 restored the previously distinct caret in each direction. The Computer Use accessibility summary continued to format Pane 1 as `focused_element`. Earlier retained captures map controller-local element 463 to Pane 1 and 464 to Pane 2, but do not expose raw UIA runtime IDs, `HasKeyboardFocus`, TextPattern caret activity, Bareline's numeric `AccessibilitySnapshot::focus`, or the controller cache age. Closing the split retired both pane providers and restored the single `Editor` provider.

The same R6 interval exposed a separate, source-confirmed history defect. Typing `Z` in Pane 2 updated the shared resident document to `one Ztwo`; after Close Split View, Ctrl+Z on either resident tab reported no undo and left the bytes unchanged.

## Source diagnosis

No source assignment explains a Pane 2 underline with a Pane 1 provider focus in one composed snapshot. Pointer body activation, `view.focus_other`/F6, and UIA provider Focus all call `ViewsRuntime::activate`. The underline, provider `selected` state, and provider focus remap all read the resulting `ViewController::active_pane()`. The Windows bridge publishes `NodeId(snapshot.focus)` directly; AccessKit derives `GetFocus`, `HasKeyboardFocus`, and its focus-change event from that active tree. The TextPattern override independently reports an active caret only when the same shared snapshot focus equals its provider owner.

The retained focus observation therefore establishes a native accessibility discrepancy but does not locate it in Bareline snapshot composition, AccessKit tree publication, or the Computer Use formatter/cache. A production change at any one of those layers would be speculative without observing the boundary value on the same frame. No focus source or golden was changed.

Resident undo/redo selection history is view local even though split panes share one document service. `EditorSurface::clone_view` previously started with empty history, and `refresh_peer` cleared the other pane's history after a shared revision arrived. `ViewsRuntime::collapse` then retired one view. The document bytes survived, but the only cursor for the shared document history could be discarded regardless of which pane remained active.

The correction keeps one owner for the shared resident undo/redo cursor and moves its existing `Vec` allocations to the pane receiving focus or input, then back to the workspace editor before collapse or secondary-view replacement. The move is constant time; it does not clone as many as the configured 1,000,000 retained history entries on every linked revision. Identity, exact revision, and idle checks fence the transfer, and the workspace editor retains recovery and file ownership. Linked snapshot refresh now rejects a busy publisher before advancing or clearing the target. Compare close now preserves the live secondary until its edit and history settle and reports the pending work through a nonmodal workspace message. A direct regression holds the publisher busy and proves the target bytes, revision, and history remain unchanged until safe transfer; split regressions cover both writer/focus directions, closing the secondary tab, and closing compare during an in-flight secondary edit, then verify exact bytes and undo/redo state.

## Allowed remaining diagnostics

1. Retain the numeric `AccessibilitySnapshot::focus` and the subsequent AccessKit `TreeUpdate.focus` for the exact Pane 2 click and each F6 direction. The existing `qa-inventory` logging points already emit both values when enabled.
2. In the same interval, query raw UIA `GetFocusedElement`, both providers' `CurrentHasKeyboardFocus`, and each TextPattern2 `GetCaretRange` active flag through an authorized independent observer. Record runtime IDs and labels together; controller-local ordinals are insufficient.
3. If snapshot and tree focus are Pane 2 while raw UIA remains Pane 1, isolate the AccessKit adapter update/event boundary. If raw UIA is correct while the formatted field remains Pane 1, classify the controller cache. Only a wrong composed snapshot justifies an application focus correction.

The existing headless provider test remains the contained guard for distinct providers, shared text, independent selection, stale action retirement, and provider retirement. Add an exact pointer/UIA/F6 focus regression only with the source correction that explains the native boundary failure.

## Validation

`git diff --check` and direct `rustfmt --check` passed. Per integration ownership, no Cargo, build, native, or UI command was run. The focused checks for the integrator are `cargo test -p bareline closing_split_preserves_shared_undo_redo_from_either_writer`, `cargo test -p bareline closing_secondary_tab_returns_its_document_history`, `cargo test -p bareline close_compare_waits_for_secondary_history_to_settle`, and `cargo test -p bareline-editor-surface linked_peer_waits_for_history_before_advancing_snapshot`.
