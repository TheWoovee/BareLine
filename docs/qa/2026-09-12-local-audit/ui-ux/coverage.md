# UI/UX coverage matrix — 2026-09-12

This matrix distinguishes current native observation, automated native journey evidence, source tracing, controller-blocked work, and checks that were not executed. Manual testing used generated scratch files and private application roots only.

| Journey | Current evidence | Result / limitation |
|---|---|---|
| Startup / initial editor | Native observed; `smoke`; `p4-2` | Pass. Fresh Untitled, editor, tab, status and menus appeared coherently. There is no separate first-run onboarding flow. |
| Menu taxonomy / submenus | Native; `p1-1`, `p1-2`; source | Reachability/taxonomy pass. Popup theme, height, density and state issues are UX-05. |
| New document / edit / dirty marker | Native; `p4-1` | Pass; one dirty marker. |
| Unicode / Arabic / combining / emoji | Native; journey conflict | Resident input/render observed. Earlier `p4-4` passed; final E3-hash suite failed color detection at saturation 165→165. Bidi editing, IME candidates and font fallback matrix remain untested. |
| Open file / cancel | Native command-line and reopen | Existing scratch files opened and bytes rendered. Native Open-dialog selection/cancel/recent-folder behavior remains unverified because child-dialog targeting was unreliable. |
| Save new resident file | Native observed; disk checked | Pass. Untitled saved as `resident.txt`; tab became clean and disk bytes matched. Save dialog opened with blank suggested name (UX-01/T03). |
| Edit / Save / reopen resident | Native observed; disk checked | Pass. Appended ` gamma`, Ctrl+S cleaned the tab, disk was exactly `resident alpha alpha beta gamma`, and later reopen showed the same bytes. |
| Save As existing file | Native observed; source | Fail: native overwrite confirmation led to conflict, unchanged destination and staged temp. UX-01, root T03. |
| Save Copy existing file | Source confirmed | Same absent overwrite-intent representation; native confirmation not repeated. |
| Save All / cancel | Native observed | Pass for cancellation: Save All opened one Save As; Escape retained dirty Untitled and reported `0 saved; 1 cancelled or closed; 0 failed or changed during save`. Multi-file success/failure mixture untested. |
| Close dirty / choices | Native observed; `p0-7` mixed | Escape retained dirty tab; Don't Save closed it and preserved saved disk bytes. Save path worked separately. `p0-7` intermittently fails prompt observation under UX-08/T09. |
| Recovery journal / crash restore | `p0-3`, `p0-5`; native center | Journey passes. Center false-empty discovery is UX-06; restore/compare/export/delete center actions remain unexecuted. Root audit owns discard implementation defects. |
| Session restore | Native observed; private session | Pass. Clean close/relaunch restored two tabs in order and the active `replace.txt`. Session is disabled by `--new-instance` or `--no-extensions`, so the validating process intentionally omitted both under a private profile. Dirty/session crash variants untested. |
| Tabs / view management | Native partial; journeys | Tab switching/close and pane-qualified tabs pass. Reorder, pin, color, MRU, selector, restore-closed and many-tab stress remain untested. |
| Split resident / paged | Native observed | Horizontal/vertical creation worked; shared edits appeared in both panes. Only one generic UIA Editor provider (UX-03/U08). Clone/move/session focus matrix untested. |
| Keyboard / mouse editing | Native partial | Basic typing, shortcuts, pointer placement and dirty state pass. Rectangular/multicaret, overwrite/numpad/dead-key/AltGr, drag/autoscroll/context actions remain untested. |
| Find resident | Native observed | Two-match query and state pass. Assistive click/type can edit the document (UX-03/U07). |
| Replace resident / Undo / cancel | Native observed | Replace All changed two `alpha` to `omega`; editor-focused Ctrl+Z restored both; Cancel reported cancelled. Replace-one/selection scope and in-flight cancel untested. Extended `[` returned `No matches`; Extended is escape processing, so this was not treated as an invalid-regex failure. |
| Find paged / paged save | Native observed; disk checked | 3,162,037-byte CRLF file found 3,097 matches. Inserted `PAGED_EDIT `, Ctrl+S cleaned tab; disk prefix and 3,162,048-byte size matched; later reopen rendered it. In-flight cancel/query replacement untested. |
| Search across open documents | Native partial; source | Surface opens. Search-mode tabs absent from UIA; result activation and multi-document replacement unverified. |
| Stale Find after tab switch | Native observed; source | Fail: paged `3,097 matches` survived switch/query change on resident file. UX-09/U09. |
| Folder search / reviewed replace | Picker reached scratch folder; controller blocked; source | Generated three-file folder and navigated picker to it. Sanctioned controller could not target the child dialog's Select Folder (cached-child unavailable/coordinate target mismatch), so search results, navigation, apply/rollback/cancel are explicitly unverified; no product failure inferred. |
| Go To | Native; `p1-5`; source | Raw shortcut opens and automated parse journey passes. UIA field/focus absent; assistive input changes editor (UX-02/U01). Manual paged target jump untested. |
| Command Palette | Native observed | Search, disabled reason, keyboard navigation/dispatch and modal semantics pass. Exhaustive command-state matrix untested. |
| Settings search | Native; `p3-6a` | Pass: focus, filtering, inert background and Escape restoration. |
| Settings change / persist / reset / revert | Native observed; source/disk | Autosaved Font size 12→9 to private `settings.toml`; confirmed Reset Section restored 12. Revert after autosave did nothing (UX-10/U10). Workspace-scope and write-error paths untested. |
| Themes / popup chrome | Native; source | Dark editor/title bar; light popup mismatch UX-05. Light/system/high-contrast and physical DPI theme passes untested. |
| Accessibility | Native UIA; source | Settings, Palette and Macro Manager strong. U01/U07/U08 cover modal ownership, search/recovery focus and split providers. Narrator announcements and full keyboard-only traversal remain untested. |
| High DPI / resize / show | `p0-4`; source | Repeated SW_SHOW passes. 125/150/200%, mixed monitors, minimum size and maximize restore untested. |
| Compare | Native observed | Two scratch documents produced one visible difference; navigation toolbar rendered. |
| Compare merge / Undo | Native observed | General options' Copy right→left changed destination, marked it dirty, reduced to zero differences, and Ctrl+Z restored it. Copy left→right, stale revisions, paged merge and save untested. Compare Options modal safety fails under UX-02/U01. |
| Macros | Native observed; source | Recorded one `x`, stopped, played selected macro, document ended `xx`, status `Macro complete`. Manager Close bug UX-07/U06. Repeat/import/export/persistence/cancel untested. |
| External command / output | Native observed | Explicit `C:\Windows\System32\cmd.exe /d /c echo BARELINE_AUDIT` confirmed, exited 0, rendered exact output and reported 0 bytes discarded. Run field accessibility fails UX-02. Cancel/timeout/nonzero/large-output untested. |
| Extensions | Parent release integration; UI disabled/empty profile | Release first-party serial integration passed 4/4 including 1 GiB JSON. Manager/catalog/install/update/cancel UI untested here. |
| Utilities / export / print | Source only | Hash/encoding/statistics, HTML/RTF export, print, cancellation and write-error UX untested. |
| Error messages / cancellation | Native partial; source | Save conflict and search/Save All cancellation observed. Duplicate/clipped errors UX-04. Permission denied, disk full, malformed encoding and worker loss not manually exercised. |
| Help / update | Menu/source only | Commands exist; documentation launch, update success/error and About details untested. |
| Deferred presentation contracts | Native/source review | Shared bottom dock, Find toggle tooltips, and non-color estimated-gutter cue remain current; briefs U11-U13. |

## Native journey runs

The initial copied-binary run was 12/13. `p0-7` failed once, then passed three isolated reruns. The fresh final suite invoked `target/debug/xtask.exe journey all` against `target/debug/bareline.exe` SHA-256 `E3F283C8CC61CCAE5D1549A1136B899D02ABF3CF95A27C9FC7BE8A42D7128702` and finished 11/13: `p0-7` again failed to observe the close prompt and `p4-4` failed `no colourful emoji glyph detected (saturation before 165, after 165)`. The other eleven passed. No retries were used to overwrite this final result. See [journey-current-binary.log](evidence/journey-current-binary.log), [journey-all.txt](evidence/journey-all.txt), and [journey-p0-7-reruns.txt](evidence/journey-p0-7-reruns.txt).

## Environment and evidence limits

The audit did not touch personal documents, clipboard contents, user profiles, or network services. The only external process was the explicitly authorized harmless System32 command. Child native picker targeting became unavailable after navigation; further folder and file-dialog work was bounded and remains unverified. Narrator, real IME candidate windows, physical mixed-DPI monitors, printer drivers, low-disk/ACL faults, extension installation, and destructive recovery actions require dedicated environments and are not claimed.
