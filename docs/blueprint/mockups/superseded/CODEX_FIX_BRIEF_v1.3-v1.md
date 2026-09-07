# Codex brief: Bareline blueprint v1.2 → v1.3 fixes

You are revising the Bareline product blueprint in `D:\Notepad_REq\Bareline_Product_Blueprint`. An independent review of your v1.2 pass found the items below. Apply every item, then verify and report exactly as specified at the end. Do not widen scope beyond this list.

## Ground rules

1. `07_DECISION_LOG.md` and `09_FOUNDATION_CONTRACTS.md` remain the authorities. Do not change any decision except where an item below says so.
2. Never delete an image or a document. A replaced file moves to `mockups/superseded/` with the next unused `-vN` suffix, and `mockups/GENERATION_LOG.json` records the supersession.
3. Bump `01`, `02`, `07`, `09`, `10`, `11`, `08`, `README.md`, `MANIFEST.json` to version 1.3 and date 2026-09-05. Regenerate manifest hashes.
4. Run `python scripts/validate_blueprint.py` at the end; it must pass. Extend it as item 22 says.
5. No em-dashes in new prose. Terse, imperative. British or American spelling, but consistent within a file.
6. Do not produce templated scoring tables. Every score you state must have a one-sentence reason specific to that file.

## A. Blocking defects for wave 0

**1. PR-001 must own the interfaces four other briefs assume it delivers.**
`09_FOUNDATION_CONTRACTS.md` FC-07 (around line 79) says "PR-001 defines `VerifiedPackageSource`, path-trust and codec interfaces". PR-004 (~line 183), PR-007 (~line 56), PR-016 (~line 69) and acceptance case AC-001-03 consume "the PR-001 codec / filesystem-capability / VerifiedPackageSource contract". PR-001's Scope, Detailed design and Likely files sections never mention them.
Fix in `PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md`:
- Scope: add three bullets: `VerifiedPackageSource` trait with a fail-closed online stub; `PathTrust` classification and canonicalization interface (local, removable, UNC/network, reparse, session-supplied, extension-supplied) with a deny-by-default policy hook; `Codec` and `FilesystemCapability` trait definitions (decode/encode streaming, BOM policy, capability report for atomic replace, ACL, ADS, hard links).
- Detailed design: add a subsection "Foundation interfaces (FC-01, FC-04, FC-07, FC-09)" with the trait shapes as Rust sketches, stating that PR-001 ships traits plus fake providers only; PR-007 supplies real codecs, PR-015 real trust evaluation, PR-018 the real online provider.
- Likely files: add `crates/platform/src/{trust,capability}.rs`, `crates/file-io/src/codec/mod.rs` (trait only), `crates/extensions-protocol/src/package_source.rs`.
- Acceptance: keep AC-001-03 as is; it now matches the scope.

**2. Move the DirectWrite/IME prototype case out of PR-001 or scope it in.**
AC-001-01 (mixed Arabic/Latin/emoji with IME candidate placement at 100/150/200 percent DPI) requires shaping, BiDi and IME caret tracking that PR-001 defers to PR-003. Choose one and make everything consistent: either (a) add a bounded item to PR-001 scope "text and input prototype: DirectWrite shaping of one mixed-script line, IME preedit overlay and candidate window positioning, device-loss recovery, throwaway code allowed" and keep AC-001-01; or (b) move AC-001-01 to PR-003 as AC-003-04 and add a replacement AC-001-01 about the startup ledger (no document read before first frame) . Update `10_ACCEPTANCE_AND_TRACEABILITY.md`, `scripts/revision_cases.json` and the affected PR "v1.2 review resolutions" section. Prefer (a): the review's own "Early uncertainty reduction" paragraph wants this prototype early.

## B. Sentences that contradict the v1.2 contracts

Replace each quoted sentence; do not just append.

**3. PR-002 ~line 77:** "cached pages must be copied/pinned before relying on them for undo" → "inverse bytes for deleted or replaced ranges are copied into owned edit/recovery segments before the transaction commits; cached or pinned view pages are never an undo strategy (FC-03)."

**4. PR-002 ~line 63 and the scope bullet ~line 36:** "one actor per open document running on the document task" → "one logical actor per open document, scheduled on a bounded shared pool (default 2 workers, configurable up to logical CPUs); an actor never owns an OS thread (FC-06, ADR-11)." Make the scope bullet say the same.

**5. `01_REQUIREMENTS.md` line ~60, principle 6:** "zero extension cost when none are enabled, including zero runtime on disk" → "zero extension cost when none are enabled: no host process, no IPC thread, no runtime memory. Installed runtime files stay on disk until the user chooses Remove runtime (FC-07)."

**6. Offset vocabulary in the shared architecture snapshot.** In all 27 PR files the snapshot bullet reads "Canonical document positions are byte offsets into the internal UTF-8 sequence; line/column/grapheme are derived views." Replace in all 27 with: "Canonical editor positions are `TextOffset` values into the valid UTF-8 text view; `RawOffset` addresses original bytes and is never implicitly interchangeable (FC-01). Line, column and grapheme are derived views." Apply the same fix to `02_IMPLEMENTATION.md` (Document position paragraph, ~line 247) and to `00_RESEARCH_BASELINE.md` line ~22 ("Internal text is always UTF-8 (ADR-02)" → "The text view is valid UTF-8 with original-byte provenance (ADR-02, FC-01)").

**7. `01_REQUIREMENTS.md` performance table, rows for 1 GB and 5 GB (~lines 464 to 465):** replace "Notepad++ does not complete comparably" with "measure completion or timeout per FC-10; do not assume failure". FC-10 forbids the current wording.

**8. `PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md` EncodingState:** rename `escaped_byte_count` to `invalid_span_count` and add `invalid_byte_count`. The escape scheme no longer exists.

**9. Search the whole package for leftovers and fix each:** `grep -rn -i "surrogate\|escape scheme\|escaped_byte\|always UTF-8\|zero runtime\|pinned\|does not complete comparably\|one actor per document\|document task"`. Report every remaining hit and why it stays, if any.

## C. Decision record and brief quality

**10. ADR-37 to ADR-46 must be real decision records.** For each, add three short paragraphs: Problem (what v1.1 got wrong), Decision (one sentence plus the FC pointer), Consequences (cost and what it replaces). Specifically record: why U+FFFD plus side table replaced surrogate escape; why disk-backed transcode replaced in-memory edit-store transcode; why inverse bytes must be owned before commit; why the pattern-string heuristic was dropped; why logical actors on a pool replaced one thread per document; why disable and remove runtime are separate; why "reproducible unsigned payload" replaced "reproducible build".

**11. Delivery slices need boundaries.** In every PR file's "v1.2 review resolutions and delivery slices" section (rename to "v1.3 delivery slices and acceptance"), replace the boilerplate paragraph with a three-row table: slice name, what compiles and is testable at the end of the slice, which AC cases or minimum scenarios prove it. Generate from `scripts/revision_cases.json` after extending that file with per-slice exit criteria. No two PRs may share identical slice text.

**12. Surface the FC-02 budgets as settings keys.** Add to PR-002 design and to `02_IMPLEMENTATION.md` §6 a table of keys with defaults: `document.resident_max_bytes` 256 MiB, `document.page_size_bytes` 1 MiB, `document.page_cache_bytes` 64 MiB, `document.aggregate_cache_bytes` 256 MiB, `undo.aggregate_ram_bytes` 128 MiB, `search.results_ram_bytes` 64 MiB, `transcode.temp_quota_bytes` min(20 GiB, 20 percent free), `clipboard.history.max_entries` 20, `clipboard.history.max_total_bytes` 16 MiB, `clipboard.history.max_entry_bytes` 4 MiB. Reference them from PR-005, PR-006 and PR-007 where each is consumed. Settings UI shows effective values (FC-02).

**13. Acknowledge the cost of the v1.2 contracts.** Add a "Sizing note" paragraph to PR-002, PR-004 and PR-007: side tables, disk-backed stores, sealed baselines and receipts roughly double to triple the v1.1 effort; list the slices that can ship first with a UTF-8-only, Resident-only path so the editor becomes usable early while Paged and legacy-codec paths land in later slices.

**14. Restore honest review scoring.** Rewrite `REVIEW_V1.2_2026-09-05.md` per-file tables (or produce `REVIEW_V1.3_2026-09-05.md` and mark v1.2 as superseded): every row gets a reason specific to that file. Remove the identical "Updated foundation contract, delivery increments and three atomic review cases" and "Unchanged pixels" lines. Where you lowered a score from the earlier review (huge-log 9 → 7.5, compare-options 9 → 7), state the specific defect that justifies it or restore the earlier score.

## D. Prompt file, UI spec and mocks

**15. Restore the concrete prompts.** `08_IMAGEN_PROMPTS.md` must again contain the full style prefixes and one complete prompt per screen (A1, A2, B, C1, C2, D1, D2, E, F1, F2, G, H, I, plus the two new states source-changed and replace-preview, plus L1 to L4). Take the thirteen from `mockups/superseded/08_PROMPTS_v1.1.md`, update them to v1.2 contracts (menu bar on every screen, six-segment status bar, no toolbar, `TextOffset` wording irrelevant to images), and add the seven executed prompts from `mockups/prompts/`. Keep the one-line brief table as a summary above the prompts. The file may state that any image model may be used; record the actual tool per image in `GENERATION_LOG.json`.

**16. Reinstate the hidden-toolbar rule for reference screens.** In `11_UI_INTERACTION_SPEC.md` line ~9 remove "workspace/compare examples may explicitly enable it". In `08_IMAGEN_PROMPTS.md` remove "optional explicitly enabled toolbar". Rule: no reference screen shows a toolbar; a single dedicated screen `bareline-toolbar-enabled.png` may show it if desired.

**17. Chrome consistency rules, add to `11_UI_INTERACTION_SPEC.md` and the prompt prefix:** title bar text is exactly "Bareline" on every screen; document tabs sit below the menu bar, never in the title bar; Settings, Extensions and Recovery Center open as tabs in that same strip, never as title-bar tabs; the standard six-tab set from A1 (main.rs active, config.toml pinned, notes.md, server.log, query.sql, Untitled 1 unsaved) is used on every editor screen unless the screen is about a different document; no "Welcome" tab exists anywhere (text-first principle); the same placeholder mark appears on every screen until the vector logo exists.

**18. Regenerate or restore these mocks. Preserve current files under superseded.**
- `bareline-settings.png`: regenerate with the setting key shown under every row (FR 5.4 hard requirement), keep the v1.2 gains (User/Workspace scope, pt unit, "All changes saved", Reset section), tabs below the menu bar, neutral status placeholders.
- `bareline-workspace-split.png`: regenerate with no toolbar, the second pane's own tab label, a visible sync-scroll icon at the divider, at least 30 lines of content in each pane, Rust file active with a Rust outline, 28 px row density.
- `bareline-compare-dark.png`: keep the v1.2 image as `bareline-compare-merge.png` (merge affordances reference) and restore `mockups/superseded/bareline-compare-dark-v2.png` as `bareline-compare-dark.png` (five-state reference) after adding the menu bar to it. Update README, NOTES and GENERATION_LOG accordingly.
- `bareline-extensions.png`: regenerate with the four tabs Installed / Discover / Updates / Disabled (FR-022), the "Extensions run isolated in a separate process" banner, plus the v1.2 gains (runtime card with Remove runtime, per-extension Permissions, human-readable capabilities). Publisher string "Bareline project" for first-party extensions.
- `bareline-recovery-center.png`: edit or regenerate to remove the Welcome tab and use title "Bareline".
- `bareline-source-changed.png` and `bareline-replace-preview.png`: edit to remove the toolbar row; everything else stays.
- `logo-brand/bareline-hero.png`: regenerate with Windows 11 chrome (minimize, maximize, close top right; the twelve-item menu bar; no macOS traffic lights), sequential line numbers, the standard six tabs, tagline only.
Score each new image with a reason, record prompts in `mockups/prompts/`, dimensions in `GENERATION_LOG.json`.

## E. Brand deliverable

**19. Pick the mark and author the vector.** Adopt the variant with two text lines inside the bowl (as in `bareline-app-icon.png` and `bareline-wordmark.png`) per `00_BRAND_AND_LOGO.md`. Write `logo-brand/bareline-mark.svg` by hand on the 24-unit grid described there (stem full height on the left, bowl in the lower 14 units, stroke 3 units, radius 1.5 units, fold at top right of the bowl, 45 degree notch of 2 units at lower right in the accent color, two text lines inside the bowl). Provide `bareline-mark-dark.svg` and `bareline-mark-light.svg` color variants and `bareline-mark-16.svg` simplified without fold, notch and lines. Add `scripts/render_icons.py` that rasterizes the SVGs to PNG at 16, 20, 24, 32, 40, 48, 64, 128, 256 and assembles `bareline.ico`, using a pure-Python or commonly available renderer and documenting the dependency. Do not claim the ICO is final until a human has viewed the 16 px result; record that in NOTES.

## F. Verification and report

**20. Cross-checks that must pass:** all 27 `Depends on` lines equal the table in `04`; mermaid equals the table; every AC ID in a PR exists in `10`; every FC referenced exists in `09`; tracker header and delimiter both eight cells; no file in `mockups/` root lacks a row in `mockups/README.md`; every image in `GENERATION_LOG.json` exists with the recorded dimensions.

**21. Regression grep must return zero hits outside `mockups/superseded/` and the two REVIEW files:** `surrogate`, `escape scheme`, `escaped_byte`, `always UTF-8`, `zero runtime on disk`, `pinned` (in undo context), `does not complete comparably`, `one actor per open document running on the document task`, `explicitly enable` (toolbar), `Welcome` (in 01, 11, prompts).

**22. Extend `scripts/validate_blueprint.py`** with the grep in item 21 as a failing check, a check that no prompt in `08` or `mockups/prompts/` for a reference screen contains the word "toolbar" except the dedicated toolbar screen, and a check that the settings keys table in item 12 appears in both `02` and PR-002.

**23. Report format.** Produce `REVIEW_V1.3_2026-09-05.md` with: a table of the 23 items above with status Done / Partially / Not done and the file:line of each change; the regression grep output; the validator output; per-image scores with reasons; and a short list of anything you judged should not be changed and why. Do not restate the whole blueprint. Do not mark any tracker row other than NOT_STARTED. Do not claim any prototype, test or benchmark ran.
