# Remaining native validation — first correction build, 2026-09-08

Executable SHA256: `8627c9f5a2081969972e9a94c7816eca30eced7931c884957c328c8559ef6fba`.

Isolated evidence root: `tests/e2e/results/remaining-20260908-fresh`. Profiles app, app2 and app3 used copied executables and portable markers. Existing QA instances and personal apps were preserved. Native inspection used Sky screenshots and accessibility snapshots; no source edits or builds were performed during this pass. Screenshots are retained in task tool history.

| Issue | Result and evidence |
| --- | --- |
| 005/011 | Partial/failing tool observation. Find visibly accepts `first`, reports one match, and preserves clean session.txt at 51 bytes. Sky aggregate focused element remains Editor and Find node lacks query value after explicit click and input. API does not expose individual HasKeyboardFocus/value properties, so provider attribution remains open. Folder query correctly reports focus and preserves document. Light options and folder controls are readable. |
| 008 | Fail. Dirty scratch Untitled containing `CLOSEQA café 🙂` (18 bytes), Ctrl+W disables owner without a visible confirmation. Exact app window enumeration shows no dialog; activation and Escape do not recover it. No/Yes branches unverified. Original app profile/process preserved. |
| 009 | Fail, with preview and byte checks passing. Saved recovery-saved.txt baseline 26 bytes was edited to 46 bytes with `RECOVERED café 🙂`; durable checkpoint baseline.bin exactly equals independent oracle. After isolated forced termination, Recovery Center shows Complete checkpoint and correct Unicode preview. Compare With Current Disk eventually paints recovered pane as `Source unavailable: Cancelled`, hatched body; disk pane correctly shows original baseline. Open recovered copy branch remains unverified. Preserved app2 checkpoint and live comparison evidence. |
| 012 | Pass. Ordinary split and compare palette accept typing, Ctrl+A, Ctrl+C and Ctrl+V in query; source remains clean, 51 and 18 bytes respectively. Palette Close Split View and Close compare commands work. |
| 015 | Partial. 20 MiB single line End reaches exact column 20971521. Painted result resolves first; UIA stays counting until second End, then reports exact column. No SelectionUnavailable/BudgetExceeded alert. Sky omits empty collapsed selected_text, so direct GetSelection contract unverified. |
| 019 | Pass in default 1202x792 light window. Left/right active tabs correctly label compare-left.txt and compare-right.txt, with matching content. Options title and Done fit before footer; underlying scrollbar does not cover options. Other sidebar/output combinations not separately exercised. |
| 028 | Pass for setup cancellation and close routes. Cancel after query preserves clean document; Close Search Results menu and Escape each dismiss folder panel; Alt+F4 with panel open on clean app3 exits. Cancellation of an actively running search worker was not separately exercised. |
| 030 | Pass for simple noncompare saved session. session.txt line 2 column 16, byte caret/anchor 26 persisted on clean exit; session.json has compare:null and split:false. Relaunch restores document and exact caret. Compare session restore not separately exercised. |
| 031 | Pass. 24 MiB paged file plus QA saved to 25165826 bytes, exactly equal independent oracle; SHA256 e5c6cbe6503989f2d72e7a4e3dc520bcbd1c1a33a09bb5288bf230e755f118e0. No false SourceChanged warning after save. Independent same-length first-byte external edit then triggers Source changed banner and external-change alert. |
| Six-digit gutter | Pass visually at line 387167. Numbers remain separate from text. Folding hit and accessibility rectangles were not separately checked natively. |
| Macro/footer | Pass via Macro Start/Stop on clean app3. Output header and status remain visible in lower 200 pixels with a single global footer, no internal duplicate footer. No external Run command was executed. |

## Recovery launch qualification

Sky launch of the preserved app2 profile failed to produce a window twice. Recovery owner launched the exact executable via controlled Start-Process, no arguments, repository working directory; PID 86696 remained alive and produced Recovery Center. Thus no application startup defect was established from the launch-tool symptom. `app2-startup-stderr.txt` preserves the startup log.

## Preserved byte evidence

`fixture-manifest.json` holds initial fixture hashes. `oracle/paged-after-QA.txt` and `oracle/recovery-after-edit.txt` are independent expected outputs. Recovery checkpoint is `app2/data/recovery/paged-52396-1788871008641584700-1`, with durable revision 2 receipt and exact 46-byte baseline. paged.txt now intentionally contains the external first-byte edit after its successful own-save oracle check.

## Outstanding acceptance

Rebuild and focused retest required for 008, 009, Find focus/value attribution (005/011), and asynchronous giant-line UIA update (015). This pass does not confirm all remaining issues resolved.

## Focused final-build retest

Executable SHA256: `f0674363759ab07d4323edba427d5386aa78765c4e03d466e2d88105a508923c`. Evidence root: `tests/e2e/results/remaining-20260908-focused`. Direct controlled launches used repository working directory, hidden process window, and redirected stdout/stderr. Sky selected returned editor windows. The preserved checkpoint was copied into a new profile; old failure evidence remains untouched.

- **008 PASS:** Ctrl+W on 18-byte generated dirty scratch text shows an owned Yes/No confirmation. No preserves text; typing `!` produces 19 bytes, proving input resumes. Yes closes that scratch tab. A separate generated EXITQA tab exercises Alt+F4: No preserves it and the application; Yes exits, verified by exact-path window enumeration returning no windows. Sky could not address one aggregated dialog element index; keyboard dialog mnemonics correctly exercised both branches.
- **009 PASS:** Recovery Center preview displays the original baseline and protected Unicode edits. Compare with current disk displays both sides correctly and reports one difference; no Cancelled/source-unavailable pane. Open recovered copy creates a new recovered tab with both lines. Save As to `recovered-copy.txt` yields exactly 46 bytes equal to the independent oracle, SHA256 `275773ebb0efa2f70ce94da95eb3fd33b6a687f205a26c882f12cf4c7e2bad75`. Original recovery-saved.txt remains unchanged, SHA256 `66498fa0d3ea1b2d43c69f90cc2dfd83f1ba037c791c1f42c6aeadec9a6e4a7f`.
- **015 asynchronous status PASS:** One End key on the 20 MiB line initially reports counting, then settles to exact UIA line 1 column 20971521 without any further key or mouse input. No SelectionUnavailable alert after load completion. Direct collapsed GetSelection remains outside the aggregate Sky API evidence.
- **005/011 application publication verified; end-to-end qualification remains:** Visible Find query `focusprobe` has 10 characters and leaves the document clean at zero bytes. `app-stderr.txt` records accepted snapshot `focus=6000 find_metadata=Some((false, true, Some(10)))` followed by active `qa_accessibility_bridge_update focus=6000`; the empty-query pair likewise publishes length zero. Sky still aggregates focus as Editor and omits the Find value. Source/bridge evidence excludes missing application publication; individual native UIA focus/value properties were not independently observed, so this is not an unconditional end-to-end UIA pass.

No passed first-build matrix was repeated beyond these focused checks. The remaining qualification concerns accessibility observation coverage, not a reproduced publication failure. Active search-worker cancellation and direct collapsed GetSelection also retain their previously stated coverage limits.
