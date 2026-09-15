# Native issue retest — 2026-09-08

Executable SHA-256: 7a72b4c9e16103e02b0888a3928ae37ff5c43957e6fbced6b6192cef4721d8f4

Testing only; no source fixes during this sweep. Original issue identifiers retained.

| Issues | Result | Evidence / actual behavior |
| --- | --- | --- |
| 001 | PASS | Fresh portable startup: dark native menu bar with branded icon and no empty Output panel. |
| 003 | PASS | Painted footer and UIA agree: 33 bytes, two lines, line 2 column 12 after multiline Unicode input. |
| 017 | PASS | Ctrl+D with empty selection selected alpha; double-click selected beta; ordinary drag selected alpha on second line. |
| 005 | UNCONFIRMED | Visible Find query accepts alpha and document is unchanged, but delayed Sky UIA snapshot still reports Editor focus and no Find/Replace value. Original observation remains; automation attribution unresolved. |
| 006 — Replace | PASS | Replace All completed both replacements; Escape from its button focus dismissed the panel. |
| 010 | PASS | Quoted three existing filenames opened sample.rs, compare-left.txt and compare-right.txt as separate tabs. |
| 018 | PASS | Show Toolbar displays buttons and all document tabs intact, with the active tab visible. |
| 012 — Open documents | PASS | Typing shared and running search produced two matches; underlying compare-right remained clean and unchanged at 23 bytes. |
| 013 | PASS | Dragged visible 24 MiB scrollbar thumb from top to near bottom: viewport moved to approximately line 375940, caret stayed line 1 column 1. Gutter digits visibly crowd text at six-digit line numbers (additional visual finding). |
| 004 | PASS | Ctrl+Shift+Home selected multiple lines; Ctrl+Shift+Right moved by a word. Ctrl+End on the 24 MiB fixture reached final line 387073 column 17. Ctrl+Shift+Home reached document start and selected the whole initial multiline document; Ctrl+Shift+Right moved the anchored boundary by a word. |
| 015 | PARTIAL | End now finishes with exact column 20971521 and renders line text after background work. UIA still raises Selection unavailable: BudgetExceeded; original indexing stall resolved but accessibility selection is not verified. |
| 031 | FAIL | Saving the isolated 24 MiB scratch file still raises Source changed; current bytes preserved without any external writer. Saved file is 25165826 bytes, ending QA. |
| 023 | PASS | Rust completion for mess selected message with Down; Tab inserted message, not indentation. |
| 011 — Language | PASS | Language picker uses light background and readable text in Light mode. |
| 022 | PASS | Applied Plain text to sample.rs with Enter, switched to compare-left and back; Plain text remained effective. |
| 014 | PASS | Discover becomes selected and shows signed offline catalog guidance. |
| 011 — Extensions | PASS | Extensions panel and tabs follow Light mode. |
| 006 — Extensions | PASS | Escape dismissed Extensions from Discover tab. |
| 024 — Print | PASS | At 1202×792 all print controls fit; Shift+Tab kept the 69-byte document clean. Bottom Choose printer button opened native Microsoft Print to PDF dialog. |
| 024 — Native modal observation | PASS WITH TOOL LIMIT | Native Print is a separate window; main-window Escape targeted the disabled owner. Activating Print and clicking its Cancel dismissed both dialogs. Earlier apparent stuck options is not a confirmed app failure. |
| 006 — Utility result | PASS | Escape dismissed SHA-256 utility result. All three original Escape subcases now pass. |
| 032 | PASS | Fresh HTML export of unsaved café 🙂 <QA> succeeded without OS32, created HTML artifact, and left source text intact. |
| 008 | BLOCKED / UNCONFIRMED | Ctrl+W on unsaved Unicode Untitled again left the owner unresponsive with no targetable confirmation. list_windows showed only editor; owner activation did not expose it. HTML copy preserves content. Native modal attribution remains unresolved. |
| 002 | PASS | Fresh Unicode editing created recovery checkpoint without OS32. After isolated forced exit, recovered copy contained exact café 🙂 <QA>. |
| 009 | PARTIAL | Recovery Center found complete new checkpoint and Open recovered copy restored exact Unicode text. Preview remained Preparing bounded preview at inspection; disk comparison is unavailable for this originally unsaved document, so saved-file preview/Compare subcase remains unverified. After clean restart, the remaining Unicode checkpoint preview reported Preview unavailable: Changed, while Open recovered copy again produced exact café 🙂 <QA>. Saved-file Compare with disk remains unverified. |
| 021 | PASS | Workspace click on already-open sample.rs activated its tab and Rust content after compare-left was active. |
| 016 | PASS | With 239 px Workspace sidebar and toolbar visible, clicking painted compare-left tab at x602 selected compare-left and its content correctly. |
| 011 — Workspace | PASS | Workspace tree follows Light mode with readable labels. |
| 012 — Folder query | PASS | Folder query accepted shared; Tab moved to File extensions without editing the underlying 17-byte clean document. |
| 028 | PARTIAL | Folder panel lower Search/Cancel controls fit default 1202×792. Tab stayed in fields and Escape dismissed the panel. Cancellation, Close Search Results and Alt+F4 while this panel is open were not separately repeated; these routes remain unverified. |
| 019 | FAIL | Comparison toolbar names sources correctly, but left pane tabs show recovered-unicode/sample.rs while displaying compare-left. Options title now visible; vertical split scrollbar paints through options and Panel bottom needs separate clipping verification. |
| 012 — Compare palette | FAIL | With comparison active, palette visually focused/selecting previous query; typing Close compare left query unchanged and inserted 13 bytes into compare-left (17 to 30 bytes, dirty). |
| 020 | PASS | Mapper rejects Ctrl+DefinitelyNotAKey with Unknown key: DefinitelyNotAKey and retains F3. |
| 024 — Column Editor | PASS | Column Editor fits default window including Apply/Cancel. |
| 011 — Column Editor | PASS | Column Editor follows light mode. |
| 027 | PASS | Recorded MACROQA insertion, played it on empty Untitled: exact seven bytes inserted and playback finished; Cancel Macro Playback disabled afterward. |
| 007 | PASS | Editing secondary clone at line 2 reported UIA line 2 column 5 and shared text correctly. Rectangular selection exposed both rows as selected_text a\no, matching visible right-pane selection. |
| 012 — Regular split palette | FAIL | The same palette input leak occurs in a regular clone split: typing Close Split View replaced both selected rectangle rows in the editor while palette query stayed unchanged. |
| 026 | PASS — EXPECTED GUARD | Open file rename reported Close the document before renaming or deleting its file. After close, rename succeeded; retained delete removed destination and Undo restored exact original 31 bytes including mixed EOLs. |
| 011 — Document List | PASS | Document List uses light background and readable entries. |
| 011 — Outline | PASS | Outline follows Light mode and displays fn main for sample.rs. Focus/value accessibility caveat remains under issue 005. |
| 011 — Document Map | PASS | Document Map background follows Light mode. |
| 025 | PASS | Clean scratch-file Unlock prompt says Stop monitoring and load the current file for editing? The tab will remain open. Yes removed following banner and retained tab. |
| 029 | PASS | Run loaded direct where.exe fixture: owned visible confirmation listed executable and literal argument, No default. Yes completed exit 0 with captured C:\Windows\System32\HOSTNAME.EXE. |
| 030 | PARTIAL | Clean Alt+F4 exited, session.json persisted five documents and seven tabs with active tab 11/caret69. Same portable executable/profile relaunched to targetable window; full restored-view details captured in final observation. No separate Save Session command was available in palette; normal exit writes the session. Restart restored seven tab entries and sample.rs content, but status showed line1/column1 despite saved caret69. Startup failure did not reproduce; caret restoration is not accepted. |
| 011 — Accessibility qualification | PARTIAL | Light-theme visual checks passed across the listed panels. Focus/value accessibility remains unconfirmed as described in 005; do not treat the whole original issue as closed. |

## Consolidated result

All 32 original IDs have a disposition; this is not a claim that every subcase passed. 22 passed in the stated scope (including expected guard 026), 3 failed (012, 019, 031), and 7 remain partial/unconfirmed (005, 008, 009, 011, 015, 028, 030). No source changes or additional builds were made by the native QA sweep.

Confirmed remaining failures: palette text edits the underlying document in split/compare views (012); compare labels remain unrelated to pane contents and the split scrollbar overlays options (019); saving the paged fixture raises a source-changed warning despite no external writer (031), although the saved bytes matched exactly.

Additional observations requiring follow-up: six-digit line numbers crowd the editor text; after Macro/Run activity the editor displayed an internal footer plus the global footer with a blank lower region. These were not assigned new original issue IDs. Saved-session startup succeeded, but caret restoration is not accepted. Recovery copy bytes succeeded while preview still reported Changed.

Automated companion: [RETEST-AUTOMATED-20260908.md](RETEST-AUTOMATED-20260908.md): 533 passed, 1 failed, 7 ignored. The remaining test failure is paged-spill directory cleanup, OS5 Access denied. No retry was run.
