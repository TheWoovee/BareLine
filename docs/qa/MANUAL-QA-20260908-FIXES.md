# Manual QA fixes — 2026-09-08

The owner authorized focused fixes after reviewing the QA report. Historical observations remain unchanged. Address issue IDs in order; ISSUE-026 is clarified expected behavior. Investigate attribution-unconfirmed findings before changing code.

Implementation uses Astra with low reasoning effort. One implementation writer at a time. No per-fix builds or test runs; perform the combined validation after all fixes land or when the owner requests it.

| Issue | State | Delivery |
| --- | --- | --- |
| 001 | Source fixed; validation passed | Silent background persistence, branded icons and theme-aware native HMENU bar. Native navigation and accessible labels preserved. |
| 002 | Source fixed; reviewed | Close synced manifest writer before sealed reopen; fixes self-sharing violation and new recovery baseline creation. Existing incomplete captures not repaired. |
| 003 | Source fixed; reviewed | Global footer now paints active-view status instead of placeholders when panels reduce editor bounds. |
| 004 | Source fixed; reviewed | Preserve Ctrl document/word navigation and Shift in both routes; macro recording/replay retains modifiers. Paged boundary verification deferred. |
| 005 | Investigation pending | Find value/focus projection already present; no speculative change. |
| 006 | Source fixed; reviewed | Extensions lifecycle pass-through and legacy Find keymap interception corrected. Utility and Find Escape passed natively in corrected builds. |
| 007 | Source fixed; reviewed | Active-pane text/status/geometry/actions and bounded multiple-selection reading/Add/Remove now agree. Runtime verification deferred. |
| 008 | Attribution unresolved | Existing owned native confirmation needs final verification; no speculative patch. |
| 009 | New-baseline cause fixed by 002 | Previously incomplete artifacts remain incomplete. |
| 010 | Source fixed; reviewed | Multi-file Open uses GetResults and preserves native UTF-16 paths. |
| 011 | Source fixed; reviewed | Language picker, Column Editor, Extensions, Document List, Outline, Workspace and Map use effective theme tokens. |
| 012 | Source fixed; reviewed | Focused fields own text shortcuts before editor dispatch; Compare yields to palette. Runtime validation deferred. |
| 013 | Source fixed; native drag passed | Resident scrollbar now has shared paint/hit metrics and native capture. Viewport moves while caret remains unchanged. |
| 014 | Source cause fixed by 006 | Lifecycle/redraw events pass Extensions. |
| 015 | Source fixed; validation passed | Bounded near-caret layout, exact cancellable background column counting and backward fragment panning. |
| 016, 018 | Source fixed; reviewed | Stop clipping away the actual translated tab strip; visual and hit geometry now share origin. |
| 017 | Source fixed; reviewed | Ordinary drag/double-click selection and bounded empty-selection Ctrl+D word seed added. |
| 019 | Source cause addressed by 016/024 | Actual pane labels/options exposed by corrected clip and Output paint ordering; native confirmation pending. |
| 022 | Not reproduced in fixed batch | Plain text applied with Enter survives switching away and back in native sample.rs; no dedicated patch claimed. |
| 024 | Source fixed; reviewed | Output yields overlay input and paints beneath dialogs; field layer prevents Shift+Tab reaching document. |
| 020 | Source fixed; reviewed | Reject unknown logical keys; normalize Arrow aliases. |
| 021 | Source fixed | Explorer pointer/UIA activation selects existing tab instead of duplicate load. |
| 023 | Source cause fixed by 012 | Completion popup owns Tab before document indent key binding. |
| 025 | Source fixed; reviewed | Stop-monitoring confirmation describes retained open tab instead of discard/close. |
| 026 | No mutation fix required | Closed-file rename/delete/undo passed |
| 027 | Source fixed; reviewed | Normal/saved macro start notifies event loop; playback deadlines merge with central idle scheduling. |
| 029 | Source ownership corrected | Run confirmation now has the editor HWND; default No and approval semantics preserved. Original automation symptom needs confirmation. |
| 030 | Standard restart passed; original profile unconfirmed | Clean exit/relaunch restored document and viewport. No dedicated source patch claimed. |
| 032 | Source fixed; regression passed | Attribute-only directory guard retains path checks and sharing restrictions; native publication regression verifies new and replacement export. |
| 031 | Source fixed; reviewed | Defer busy/stale watcher checks and clear stale conflicts after fresh unchanged check. |
| 028 | Source fixed; reviewed | Correct close/cancel target, focused-field routing, and Output-overpainting fix. |

See [batch validation](MANUAL-QA-20260908-FIX-VALIDATION.md) for automated and focused native evidence. Preserve the pre-existing `.gitignore` modification and untracked `.ignore`.
