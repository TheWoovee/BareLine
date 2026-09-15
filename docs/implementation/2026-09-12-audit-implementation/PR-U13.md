# PR-U13: Estimated gutter cue

- **Baseline:** `d5d35cee8fa0668bc288b3383a797b626d43e76e`
- **Scope:** Carry exact versus estimated line-label state through paged viewport painting, add a compact non-color approximation cue, and expose the transition through status and accessibility.
- **Constraints:** Preserve global line calculations, byte-based navigation, bounded indexing, and existing renderer interfaces.

## Evidence

- Paged views now carry explicit estimated/exact gutter state tied to a known terminal line count in the retained sparse index for the same document/content generation. Progress percentages remain presentation only and cannot establish exactness. A change only requests a repaint; it does not invalidate shaped text, alter byte navigation, or rebuild global mappings.
- The line worker publishes short-lock, generation-qualified progress/count receipts while retaining exclusive ownership of the mutable sparse index through page waits. UI pump/status reads use a nonblocking receipt observation and preserve the last matching terminal receipt during brief publication contention. New generations filter old receipts, and cancellation only touches the active atomic owner.
- While incomplete, every visible label keeps its verified global number and paints a separate `~` cue in a fixed gutter column plus the existing muted color. The fixed column preserves numeral alignment at scaled DPI and does not depend on italic font availability.
- The status strip says line numbers are estimated and prefixes its line value with `~`. Accessibility describes the visible gutter range and caret line as estimated; the stable status nodes change to exact text once when the index reaches completion.
- `peer_tests::go_to_end_resolves_to_the_last_line_once_the_index_is_complete` now holds an actual scan after mutable-index acquisition and proves UI pump, progress/status reads and cancellation finish before release. It then proves an index above 99.9% remains estimated until EOF establishes its line count, preserves exact state during a held receipt publication lock, and verifies the same row loses the marker, retains its global number, reports the exact count, and does not repeat the transition. Installing a later complete resident viewport slice must preserve the full paged index's exact state.
- The existing streaming-prefix workspace fixture now checks the current rendered contract instead of the retired generic `indexing…` copy: the incomplete same-tab prefix is read-only and renders `Line numbers estimated · indexing` plus the `~` gutter cue; the completed snapshot is writable, preserves document identity and path, renders exact tab status, and removes both estimated cues.
- Static checks in the private worktree: direct `rustfmt --edition 2024` on all four changed Rust files and `git diff --check`. Cargo remains central by policy.

## Proposed contained integration command

`cargo test -p bareline-editor-surface peer_tests::go_to_end_resolves_to_the_last_line_once_the_index_is_complete --offline --locked`

## Deferred qualification

- Light, dark, high-contrast, and 100/150/200% DPI visual checks remain part of final native qualification.
