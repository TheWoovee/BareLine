# Bareline UI Interaction Specification

**Version:** 1.3 • 2026-09-05 • **Status:** specified; interactive prototype and usability evidence pending.

The requirements and foundation contracts determine behavior. PNG mockups illustrate composition and hierarchy; they are not editable control trees or proof of accessibility. Use this document with [design tokens](mockups/design-tokens.json), [current mockups](mockups/README.md) and [acceptance cases](10_ACCEPTANCE_AND_TRACEABILITY.md).

## Shared chrome and component rules

Use title bar text exactly "Bareline" on every screen. Place one consistent placeholder b mark in the title bar until the vector mark is integrated. Put document tabs directly below the twelve-item menu bar, never inside the title bar. Open Settings, Extensions and Recovery Center as tabs in that strip. Use the standard six-tab set on editor screens: config.toml pinned first, main.rs active, notes.md, server.log, query.sql and Untitled 1 with an unsaved dot; change the active document only for document-specific screens. Start in text; omit greeting or dashboard tabs. Label each split pane with its own document.

A normal Windows window has a distinct title bar with minimize/maximize/close, then menus in this order: File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help. Hide the toolbar in every reference screen. Permit it only in a dedicated bareline-toolbar-enabled.png reference. Place tabs directly below the menu bar, with pinned tabs first by default. Each split pane has an independently labelled active document. Global tab and pane focus must not imply that one file's content belongs to another.

The status bar has six semantic groups: language; size and line-count completeness; line/column; EOL; encoding; insert/overwrite. Unknown values display indexing or unavailable, never guessed exact values. Settings/recovery pages retain the active editor context only when an editor exists; otherwise show neutral placeholders. Generated footer numbers are illustrative and must not be copied as fixture truth.

Every interactive control has an accessible name, role, value, state and command ID. Icons require tooltips and keyboard equivalents; errors include an icon/text description. Use native menus/dialogs through the platform adapter. Custom controls share PR-023 primitives. Focus is a 2 px outline in focus.ring; important control boundaries use border.interactive rather than the decorative border.

At 100% DPI use 13 px UI text, 12 pt editor text (16 logical pixels at 96 DPI), 34 px tabs, 28 px list rows and 24 px status bar. Point values remain points in settings. At narrow widths move secondary actions into labelled overflow menus; do not truncate the primary recovery/save action. At 200–250% DPI allow panels to stack. Long file names show a disambiguating path tooltip and an accessible full name.

## Keyboard and focus contract

Tab/Shift+Tab move within the active layer; arrows navigate lists and menus; Enter activates the focused action. Escape closes the topmost popup first, then a transient palette/find field, returning focus to its invoker. Escape in the editor never silently discards text, stops monitoring, clears a saved search or closes a document. Persistent panels close through their close command. Ctrl+F focuses find; Ctrl+H exposes replacement; Ctrl+Shift+F opens folder search; Ctrl+Shift+P opens the command palette. Shortcut overrides are user-scoped and conflict-labelled.

IME preedit remains an overlay until committed. AltGr, dead keys, combining characters and mixed BiDi text must be verified on the real Windows renderer. Screen-reader selection/text-range requests are bounded. Progress announcements are throttled; a completion or important error is announced once, not for every streamed batch.

## UI-ED Editor and split workspace

| State | Visible behavior | Action and transition |
|---|---|---|
| Loading | First available viewport; indexing indicators; no fabricated total lines | Cancellation closes the pending operation and preserves prior tabs |
| Clean | No dirty marker; selected pane drives status and outline | Edit creates a new content state |
| Dirty | Dot and accessible “unsaved” state | Save captures a state; later typing stays dirty |
| Saved-state undo | Same content state as last successful save | Dirty marker clears even though Revision increased |
| Split | Both document names visible; independent selections and scroll | Clone shares document bytes, move transfers the view |
| Provisional syntax | Subtle “Styling…” indication when context is unresolved | Valid checkpoint updates only matching revision |
| Unavailable region | Hatched region, textual explanation, unknown line hints | Follow UI-SC, never synthesize empty replacement text |

Explorer selection, tab label, code language, outline and status describe the same active document. The new workspace concept fixes the earlier TOML/Rust mismatch; the implementation must also add the independently labelled second-pane tab where the raster omits it.

## UI-FN Find, regex and replacement

Modes are Literal, Extended and Regex; scope is Selection, Current document, Open documents or Folder/workspace. Mode and scope are always visible before replacement. Case, whole word, wrap and dot-matches-newline are named toggles with persisted user preferences. Replace controls do not change search scope implicitly.

| State | Message | Enabled actions |
|---|---|---|
| Empty query | Type to find | Query input only |
| Running | Searching… with scanned amount and Cancel | Navigate already validated results |
| Complete | Exact match count and scope | Replace selected/all when revisions match |
| No results | No matches in the selected scope | Change query or scope |
| Invalid regex | Error location and plain-language diagnostic | Edit query |
| Limited / unsupported streaming | Results incomplete with reason | Refine query; replacement disabled |
| Cancelled | Search cancelled; displayed results are partial | Restart; replacement disabled |
| Stale | Source changed since search | Refresh; stale replacement disabled |

Result excerpts are bounded and grouped by file. Clicking a result revalidates/remaps its position; it never jumps by an obsolete byte offset silently. Mark styles use glyph/pattern differences as well as color.

## UI-RP Workspace replacement

Preview captures selected matches, original fingerprints and open-document revisions. A complete preview shows counts per file and an exact Before/After excerpt. Apply names the number of eligible files. A file changed since preview is unchecked, disabled and labelled “Changed since preview”; Refresh preview requires the user to review the new version. Never rematch and mutate unreviewed new content under an old approval.

During apply show per-file progress and Cancel. Cancellation stops between safe file commits; already committed files remain listed. Completion summarizes changed, skipped, failed and cancelled files, with durable receipt and unique backup locations. Undo across separate disk files is not advertised as globally atomic. Open-document linked replacement uses the separate transaction contract in FC-06.

## UI-CP Compare and merge

Label both source documents and their origin: open unsaved document, current disk or recovered copy. The selected side drives status. Toolbar includes Previous/Next difference, count, Swap, Options, Recompare, Pause automatic recompare and Close compare. Pause here is distinct from tail pause.

Each current hunk exposes “Copy left to right” and “Copy right to left”; tooltips name the destination. Copy creates a normal undoable edit and does not save automatically. A dirty destination shows Unsaved. Disabled merging explains stale revision, unavailable source or unresolved comparison.

Options use one whitespace mode (Significant, Trim edges, Ignore all), plus independent ignore case, blank lines, EOL, encoding/BOM and tab normalization switches. Coarse result quality is explicit. Cancelled/Unavailable is never labelled a completed coarse comparison. Spacers have no line numbers and never become document text. Added, removed, changed, moved/aligned and current states use non-color markers. The simplified revised raster shows one changed hunk; the options and five-state specification cover the other states.

## UI-RC Recovery Center

Complete means a full reconstructed copy is available from a sealed baseline or verified original. Edits only means saved edit data exists but complete original content cannot be reconstructed. Corrupt tail means only the validated journal prefix is usable; Source unavailable identifies missing baseline data.

Selecting a row previews that recovery without opening or overwriting the original. Open recovered copy creates a separate unsaved document. Compare with disk labels both origins. Edits-only rows offer Export saved edits and gap report; they never offer ordinary complete-file recovery. A general help card explaining Edits only must be visually identified as help, not confused with the selected Complete row's status.

Discard recovery opens a confirmation naming the document and last protected time; Cancel is default. Confirmation durably retires references before cleanup. Recovery failures show the last protected revision/time and actual limitation. Do not promise full recovery until the baseline is ready.

## UI-SC External conflict and unavailable source

For a complete dirty snapshot, offer Compare, Reload with confirmation, Keep editing and Save Copy. Ordinary Save revalidates the destination. When original regions are missing, disable Save and Save As; banner reads “The file changed outside Bareline. Some original data is unavailable.” Offer Reload… and Export available data…. Export includes owned fragments, offsets and a gap report. Reload confirms loss of unsaved in-memory edits.

Protected status means flushed recovery data and includes its timestamp. An in-memory-only edit is labelled “Not yet protected”. Unknown size/line counts remain unknown. Explain the unavailable area with text and hatching, not only an ellipsis or color.

## UI-TL Tail

Following shows a lock and “Following new content”. Pause freezes automatic scrolling while ingestion continues. Resume jumps to the newest available content. Rotation/truncation has a distinct notice and continuity handling. Unlock confirms stopping monitoring and captures a fixed generation for editing; it is not the same action as Pause. Source continuity uncertainty follows UI-SC.

## UI-ST Settings

User and Workspace scope is always visible. Workspace overrides accept the FC-09 preference allowlist and cannot grant network/process/extension permissions. Valid changes autosave with “All changes saved”; invalid input stays local with an inline reason. Storage failure says “Changes not saved” and offers Retry. Reset section previews the scope it affects. Font size is in pt. Theme contrast is checked against the actual rendered/composited background, including selection and diff overlays.

## UI-EX Extensions

States: runtime absent, download/verification pending, installed but stopped, running, permission denied, crashed and quarantined. Enabling an extension first shows runtime download size and requested capabilities. Offline Install from signed file verifies the same trust rules. A failed verification never exposes an Enable action.

Disabling all extensions stops the host and retains installed runtime files. Remove runtime is explicit, names its disk effect and stops the host before removal. Permissions are per extension. CPU/memory failure disables the offending extension and shows Retry/Keep disabled. Host-wide failure reports which extensions stopped.

Hex original-byte mode labels the original source generation and excludes unsaved text edits. A separately named encoded preview represents edited content. XML/Hex panel access is declared. Publisher/version strings in generated concepts are sample content, not a verified release identity.

## UI-UP Updates, encoding and remaining visual coverage

Update states include up-to-date, offline, verification failed, staged, restart/apply and rollback outcome. The editor remains usable when an update cannot be verified; installation stays disabled. Encoding conversion shows offending ranges, target codec and explicit loss choice; Cancel preserves the existing state. Disk-full recovery shows last protected time and a persistent Retry/settings action.

These states, high contrast, light-theme failure states and screen-reader workflows are specified but do not yet have a full interactive prototype or raster for every combination. PR-021 must close that evidence gap before product acceptance.

