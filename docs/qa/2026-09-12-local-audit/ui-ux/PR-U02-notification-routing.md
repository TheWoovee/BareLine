# PR-U02 — Route status, progress, warnings, and errors through one presentation path

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Every changed `workspace.message` is displayed in the existing message region and copied into the toast stack. Routine recovery progress and Save As conflicts therefore appear twice. Long messages and paths are clipped in both locations, and severity is guessed from English words.

## Scope

- `apps/bareline/src/windows_app.rs` (`on_redraw`, frame/message rendering)
- `apps/bareline/src/windows_app/toast.rs`
- `apps/bareline/src/windows_app/recovery.rs`
- Producers that set `workspace.message` for lifecycle/search/watch errors
- Accessibility alert/status construction in `windows_app/accessibility.rs`

## Ordered implementation

1. Replace the plain message string at the shell boundary with a typed notification `{id, level, kind, text, details, document, lifetime}` while retaining a compatibility adapter for existing producers.
2. Define routing: scoped progress/status occupies the status region; success/info may use a transient toast; warnings/errors use a persistent toast/alert with a details action. Never render the same id in two visible places.
3. Remove redraw-time string ingestion. Producers enqueue a notification once; the renderer consumes immutable notification state.
4. Replace substring severity classification with the producer-supplied level.
5. Let long errors wrap to a bounded multi-line height and expose full text/details through UIA. For paths, provide a non-clipboard-dependent “Show location” or details action.
6. Preserve per-document cleanup and bounded history. Dedupe by stable id plus revision, so identical later events can be announced again after resolution.
7. Make recovery preparation a scoped progress status and replace/clear it when complete or failed.

## State, error, and race handling

- Late UI-only progress retains its originating document key and is discarded when that document closes. Save/commit conflicts, recovery-cleanup failures, and process outcomes move to persistent operation ownership/global diagnostics and survive tab closure until acknowledged or resolved.
- Progress completion atomically replaces its progress notification.
- Errors persist until dismissed/resolved; dismissing visual UI also removes its accessibility alert.
- Repeated redraws and tab switches must not replay announcements.

## Tests and acceptance

- Render tests assert one visible representation for recovery progress and one for a save conflict.
- A long staged-copy path is fully available through UIA and readable through details without resizing the window.
- Severity tests use typed levels; wording changes do not alter level.
- Native journey observes exactly one accessibility alert per error id and no duplicate visual string.
- Closing one document clears only stale UI-only progress. A data-safety warning produced before or after close remains available globally with its operation identity.

## Risks and dependencies

Many producers currently assign strings directly. Migrate incrementally behind the adapter, then remove the adapter once all callers are typed. Coordinate save-conflict wording/details with root `PR-T03`.

## Definition of done

Each event appears and is announced once, progress resolves cleanly, long actionable content is retrievable, and severity does not depend on English text.

Status: Planned. Priority: P2. Finding: [UX-04](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
