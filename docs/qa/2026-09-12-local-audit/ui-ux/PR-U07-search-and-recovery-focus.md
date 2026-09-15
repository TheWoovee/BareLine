# PR-U07 — Complete Search and Recovery Center semantics

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Find/Replace fields are visible in UIA but `Editor` remains the reported focus; UIA clicks followed by assistive text can edit the document instead of the field. The visible Find/Replace/Files tabs are absent. Recovery Center publishes controls while leaving background editor/tabs active and focused.

## Scope

- `apps/bareline/src/windows_app/accessibility.rs`
- `apps/bareline/src/windows_app/recovery.rs`
- `apps/bareline/src/windows_app/search.rs` and `search/*`
- `crates/app/src/find.rs`
- `crates/app/src/search_panel.rs`

## Ordered implementation

1. Consume PR-U01's focus descriptor for Find/Replace, open-document/folder search, and Recovery Center.
2. Make the active Find/Search text field the snapshot focus and text provider; route UIA SetValue/TextPattern to that same field.
3. Publish Find, Replace, and Files as one tab list with selected state and Invoke/Selection actions; expose disabled reasons for unavailable modes.
4. Give every result row a stable name containing file/document, line, and excerpt; expose selection and default navigation actions.
5. Treat Recovery Center as an inert-background surface. Focus the selected row, first enabled action, or Close; keep preview status and destructive-action availability explicit.
6. Restore the precise invoker when either surface closes, and announce mode/result/loading changes once.

## State, error, and race handling

- Field ownership follows visual focus during async result publication.
- A late result may update status but cannot steal focus or revive a closed surface.
- Removed recovery/search rows clear stale selection and select the nearest survivor.
- UIA input during close returns unavailable and never falls through to the editor.

## Tests and acceptance

- UIA click/SetValue on Find and Replace changes the visible field, never document bytes, and reports that field focused.
- Search tabs are discoverable, selectable, and switch their corresponding panels.
- Result rows expose full source identity and navigate through Invoke.
- Recovery Center disables all background nodes and restores editor focus on Close/Escape.
- Provider tests assert exactly one focused node across search-mode and recovery transitions.

## Dependencies and risks

Depends on PR-U01. Coordinate loading announcements with PR-U05 and notification routing with PR-U02.

## Definition of done

Assistive input always reaches the visible search field, every visible search mode is discoverable, and Recovery Center has complete modal focus and action semantics.

Status: Planned. Priority: P1. Finding: [UX-03](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
