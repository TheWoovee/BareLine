# PR-U04 — Make File and Search menus fit, reflect state, and expose mode selection

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

At 1200×800, File requires popup scrolling and clips its last action. Search presents roughly thirty actions, including cancel/close/apply commands that are irrelevant in the current state. Several search modes do not visibly communicate their selected state.

## Scope

- `crates/app/src/menus.rs`
- Command registration/state annotation in `apps/bareline/src/windows_app.rs` and search/lifecycle modules
- Native menu builder state mapping
- Command-palette categories remain unchanged except for corrected states/labels

## Ordered implementation

1. Inventory File/Search commands by frequency and state. Keep primary document actions at top level; group recovery/advanced disk controls and search marking/scope/mode actions into short, meaningful submenus.
2. Keep all commands palette-reachable. The menu change is presentation hierarchy only.
3. Annotate cancel/close/apply commands as not applicable when no corresponding operation/surface/result exists. Disable rather than hide commands whose stable placement aids learning.
4. Represent literal/extended, scope, match-case, whole-word, and similar modes as checked/radio state. Ensure the native menu builder displays that state.
5. Add a viewport-fit invariant for the supported minimum/default window height; no first-level popup should require scrolling at 100% DPI.
6. Review labels for task intent and consistency (`…` only when a following choice/input is required).

## State, error, and race handling

- Menu state is recomputed immediately before opening and after async search/save state changes.
- Cancel remains enabled until cancellation is acknowledged, then disables.
- Apply Reviewed Replacements requires a current, non-stale reviewed result set; stale state gets an explicit disabled reason in the palette.

## Tests and acceptance

- State-table tests cover idle/running/cancelling/complete/stale search and save-all states.
- Structural test measures item counts/estimated heights for 600, 768, and 800 logical-pixel windows.
- Native journey verifies no File/Search scroll arrows at the default window size.
- Native journey verifies checked/radio state updates after toggling each search mode.
- Existing taxonomy/reachability journeys continue to pass.

## Risks and dependencies

Moving menu paths can affect users' learned locations and documentation. Preserve command ids and shortcuts, update docs once, and keep the palette as the exhaustive route.

## Definition of done

File and Search fit the supported viewport, idle-state menus do not advertise usable cancellation/application actions, and every active mode is visible through native checked/radio state.

Status: Planned. Priority: P2. Finding: [UX-05](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
