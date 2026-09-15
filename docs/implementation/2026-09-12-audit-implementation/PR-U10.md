# PR-U10 — Settings Revert opening-session baseline

Baseline: `91cc150ceb2e72c61208bbc7c155e5f231ee246c`.

## Scope

Make Revert restore the selected scope to the state captured when Settings opened, even after successful autosaves. User and Workspace keep independent baselines. Reset Section remains a confirmed override removal and can be undone by Revert.

## Required behavior

- A successful write advances persistence state without advancing the opening-session baseline.
- Revert is enabled only when the current effective scope differs from its opening baseline. It applies that baseline to live consumers and queues one generation-fenced save.
- A late save acknowledgement cannot replace reverted values. Write failure keeps Revert retryable and reports one truthful failure state.
- External file changes require an explicit conflict or reload decision and never silently redefine the session baseline.
- Closing and reopening Settings captures fresh per-scope baselines. Scope switches preserve each scope's own baseline.
- Help and accessibility state describe the opening-session scope of Revert.

## Visual contract

Preserve the current Settings tab, visible User/Workspace scope, category list, setting rows, saved/error status, and separate Reset Section action from `bareline-settings.png`. U10 changes state and explanatory text without redesigning the page.

## Evidence plan

Focused deterministic tests will cover autosave then Revert across UI/runtime/disk, Reset Section then Revert, delayed/failing generations, independent scope switching, close/reopen, and external-edit conflict handling. Root owns contained Cargo and native qualification.

## Final integration correction

- The split-pump compare failure regression now verifies the authoritative persistent typed notification, including the exact peer error and retained-recovery consequence, after both tracked request outcomes are drained. It no longer expects the retired workspace message channel to duplicate that notification.
