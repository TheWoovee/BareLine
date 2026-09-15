# PR-U12 — Find and Replace toggle tooltips

## Baseline

- Base commit: `04e4bb28fbd416ebf9aad3ef67183ed3aec48707`
- Dependency: PR-U09 document-bound Find state, including its reviewed scope/cancellation correction, and U07 accessibility ownership.
- Worktree: `audit-implementation-worktrees/modal`.

## Scope

Add delayed, clamped tooltips for Match case, Whole word, and Extended escape sequences. Visible tooltip wording and UIA name/selected state share one contract. Reuse the existing deadline wake path and preserve the current Find composition.

## Decisions

- Keep the existing compact toggle composition from `bareline-find-bar.png`; the tooltip appears below the complete Find/Replace surface so it does not cover either active text field.
- Use the shared delayed `Tooltip` primitive and the Windows event loop's next-deadline control flow. Each hover schedules one 500 ms wake, and the wake is consumed before painting so an idle window does not continuously redraw. Cursor exit and window focus loss dismiss before modal or panel event capture.
- Derive visible tooltip text and UIA names from one contract. Extended mode names all supported slash, null, newline, return, tab, and hexadecimal Unicode escapes; toggle paint, UIA selection, and command checked state agree. The shared primitive paints lines wrapped from actual backend measurements so every escape stays visible at narrow widths. A short Replace viewport uses the concise function name while UIA retains the full help, or suppresses the popup when no field-safe space remains.
- Compute tooltip geometry in logical pixels, cap it to the editor viewport, and clamp both horizontal edges with an eight-pixel inset. Find drawing and hover hit testing share the same vertical-tab inset and content width. This produces the same logical result at 100%, 150%, and 200% DPI.

## Changed files

- `crates/app/src/find.rs`
- `crates/app/src/workspace.rs`
- `crates/ui/src/overlays.rs`
- `crates/renderer/src/lib.rs`
- `apps/bareline/src/windows_app.rs`
- `apps/bareline/src/windows_app/views.rs`
- This implementation note.

## Focused checks

- `cargo test -p bareline-app toggle_tooltips_share_names_and_pressed_state_with_semantics`
- `cargo test -p bareline-app toggle_tooltip_deadline_wakes_once_and_dismisses_on_leave_or_close`
- `cargo test -p bareline-app tooltip_content_wraps_and_clamps_without_covering_the_query`
- `cargo test -p bareline-app short_replace_view_uses_summary_or_hides_without_covering_a_field`
- `cargo test -p bareline vertical_tabs_share_find_draw_and_hover_geometry`
- Cargo, semantic golden capture, and native desktop qualification are owned by root integration; no Cargo or desktop run was made in this isolated worktree.

## Unresolved qualifications

- Native pointer timing and DPI journeys remain for final integrated qualification.
- Shared Find/Search dock composition remains assigned to PR-U11.
