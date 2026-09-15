# Native journeys (`cargo xtask journey`)

The journey runner (`xtask/src/journey.rs`, P5-9) launches the built
`bareline.exe` in a throwaway data directory, drives it with `SendInput` and
native menu commands, captures window screenshots, and asserts on pixel regions
and on the diagnostics log. Each journey encodes one acceptance check from
`docs/implementation/PLAN-20260908-REVIEW-FIXES.md` and is small and independent:
it starts its own process, does a few steps, and is torn down by killing only the
process it started. Windows only.

## Running

```
cargo xtask journey <name|all> [--build]
```

- `<name>` runs one journey (see the list below); `all` runs every journey in
  order and fails if any journey fails.
- `--build` runs `cargo build -p bareline` first. Without it the runner uses the
  existing `target/debug/bareline.exe` and errors if that binary is absent.
- Output: one `PASS <name>` / `FAIL <name> — <reason>` line per journey on
  stdout; a non-zero exit if any journey failed.

The acceptance target for P5-9 is that `cargo xtask journey all` passes locally.

### The session must be unlocked

The interactive journeys need a real, unlocked interactive desktop: `SendInput`,
foreground focus, and screen-capture only work against the live session. On a
locked workstation `LogonUI` holds the foreground and a capture returns the lock
screen, so those journeys will fail (and a capture would expose the user's lock
photo). Run journeys only on an unlocked session. The `smoke` journey is the
exception — it renders a hidden frame offscreen and needs no desktop.

## Isolation

Each journey creates `target/journey/<name>-<pid>-<stamp>/` and points
`LOCALAPPDATA`, `APPDATA`, `TEMP`, and `TMP` at it, so the app's data root
becomes `…/<home>/Bareline` and never touches the developer's real profile
(settings, session, recovery journals, macros, diagnostics). Every process is
launched with `--software --no-session --no-extensions --new-instance` for a
stable, non-interactive configuration. Screenshots are written as 32-bit BMPs
under `<home>/shots/` for post-mortem review (no image-encoder dependency).

## Input handling (reference: `docs/qa/evidence/review-20260908/native-driver.ps1`)

The review's PowerShell driver established two facts that the runner follows:

- **Plain character typing reaches the window; synthetic modifier chords do
  not.** So text is entered with `SendInput` `KEYEVENTF_UNICODE` (which delivers
  `WM_CHAR`, surrogate pairs included), and commands that would otherwise need a
  chord (Ctrl+G, Ctrl+W, Ctrl+,) are invoked through their menu entries instead.
- **Menu commands work.** `invoke_menu` walks the native `HMENU` by label,
  resolves the item id with `GetMenuItemID`, and posts
  `WM_COMMAND(id, lParam=0)` — exactly the shape the app decodes as a menu
  selection (`crates/platform-windows/src/native.rs`). The menu carries the same
  accelerators (Ctrl+G etc.), so this exercises the same command path.

Focus mirrors the driver's `ForceFg`: a short synthetic Alt tap, then
`SetForegroundWindow`. A `chord()` helper exists for best-effort virtual-key
chords but is intentionally unused for command invocation.

## Encoded journeys

| Name    | Plan item | What it asserts |
|---------|-----------|-----------------|
| `smoke` | —         | Launches with `--smoke`, prints a `first_frame` event, exits cleanly. |
| `p0-3`  | P0-3      | Typing into Untitled keeps ≤ 2 recovery directories; discard on close leaves 0. |
| `p0-4`  | P0-4      | Ten external `ShowWindow(SW_SHOW)` calls leave the window visible with a real rect; the diagnostics log records a `first_frame`. |
| `p0-5`  | P0-5      | Kill with unsaved text, relaunch into the same home; a recovered document is presented (window title check). |
| `p0-7`  | P0-7      | File ▸ Close on a dirty tab shows a prompt with Save / Don't Save / Cancel buttons. |
| `p1-1`  | P1-1      | No submenu holds exactly one command whose title equals the submenu. |
| `p1-2`  | P1-2      | Top-level menu bar is the curated taxonomy with no Utilities/Workspace/Extensions top-level menus. |
| `p1-5`  | P1-5/U01  | Search ▸ Go To Line and Run ▸ Run open their prompts; one Escape dismisses each empty prompt. |
| `p3-1`  | P3-1      | Toggling a dock panel changes the body without collapsing the window. |
| `p3-6a` | P3-6a     | Settings ▸ Preferences opens the Settings surface (visible change). |
| `p4-1`  | P4-1      | Making the document dirty shows a marker in the tab strip; the caption has no doubled bullet. |
| `p4-2`  | P4-2      | Closing the sole (clean) tab keeps the app alive with a titled window (fresh Untitled). |
| `p4-4`  | P4-4      | Typing colour emoji produces saturated (colourful) pixels in the editor. |

Pixel-region journeys (`p1-5`, `p3-1`, `p3-6a`, `p4-1`, `p4-4`) assert that the
driven action produced a visible change (or, for emoji, colour saturation) in
the relevant region rather than matching an exact golden image, so they are
robust to theme and DPI. The strongest signals — window state (`p0-4`), native
menu structure (`p1-1`, `p1-2`), on-disk recovery journals (`p0-3`), the modal
prompt's child buttons (`p0-7`), and the diagnostics log — are checked directly.

## Adding a journey

1. Write a `fn journey_<id>(env: &Env) -> Result<(), String>` in
   `xtask/src/journey.rs`. Use `Session::launch(env, "<id>", extra, files)` to
   get an isolated process; the returned `Session` kills the process on drop.
2. Acquire the window with `session.wait_window(timeout)`, then drive it:
   `type_text(...)` for characters, `invoke_menu(hwnd, &["Menu", "Item"])` for
   commands, `shot(&session, hwnd, "label")` to capture. Assert on
   `Image::diff_fraction` / `Image::max_saturation`, on `session.diagnostics()`,
   on `session.recovery_dir_count()`, or on window/menu queries.
3. Add a `Journey { name, summary, run }` row to `journeys()` and a row to the
   table above. Prefer the most robust signal available; keep the journey small
   and independent.

## App hooks the runner relies on

All present in `apps/bareline/src/windows_app/launch.rs`; nothing is missing:

- Isolated data root via `%LOCALAPPDATA%`/`%APPDATA%` (installed mode joins
  `Bareline`); `%TEMP%`/`%TMP%` for the per-run cache sweep.
- `--smoke` (hidden render + `first_frame` on stdout, then exit), `--software`,
  `--new-instance`, `--no-session`, `--no-extensions`.
- Diagnostics JSON-lines at `<home>/Bareline/diagnostics/bareline.log`
  (`first_frame`, `idle`, `startup_action`, …) for non-smoke runs, and
  `handles.log` under `--diag handles`.

TODO (not blocking): there is no CLI flag to force a specific data directory
directly — the runner sets it through the environment, which is sufficient. If a
future journey needs a chord that the menu cannot reach, the app would need
either a test-only command hook or a reliable synthetic-input path into the
winit window (synthetic chords currently do not reach it).
