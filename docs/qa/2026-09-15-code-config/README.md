# DEV-003 code_config — Focused results

## Outcome

**All three code_config native steps pass, followed by checked editor Exit 0.** The procedure now opens generated Rust, checks the selected language and four actual token colors, uses native completion and indentation, verifies one completion Undo/Redo, and saves/closes/reopens exact UTF-8/LF bytes. Surrounding Unicode text and unsaved disk content are checked explicitly.

The native run exposed and fixed an app defect: render_frame published accessibility while on_redraw had temporarily removed the renderer from Shell. That produced empty text-range geometry despite visible syntax. Publication now follows renderer restoration. The passing run observes 2 keyword, 5 string, 2 number and 7 comment rectangles; expected-color pixel counts are 22, 37, 23 and 54 respectively, with zero ordinary-text-color matches in those probes.

## Checks

| Check | Result | Receipt |
|---|---|---|
| Shared native adapter | 22 Python tests pass | [Final adapter tests](captures/focused-tests-final.json) |
| Code/config observation oracle | 10 Python tests pass | [Final oracle tests](captures/code-oracle-tests-final.json) |
| Pixel sampler | 10 synthetic bitmap checks pass; no desktop/app accessed | [Sampler receipt](captures/visual-sampling-tests.json) |
| Incremental debug build | Exit 0; 7.403 seconds | [Build receipt](captures/debug-build.json) |
| Native code_config | s1/s2/s3 PASS; clean Exit 0 | [Final native receipt](captures/native-code-config-r5.json), [observations](runs/native-code-config-r5/scratch/native-observations.json) |

The oracle tests reject missing/duplicate checkpoints, wrong or absent token colors, missing geometry, bad pixel counts, an unselected completion, changed surrounding text, wrong line endings/bytes, missing screenshots and altered final files. Native PASS is based on actual checkpoints; the synthetic tests never establish product acceptance.

The [captured client image](runs/native-code-config-r5/scratch/code-highlighting.png) was visually inspected. It contains the owned editor client area only. Unfold All is invoked before inspecting visible tokens; opening the fixture initially collapsed its function. The fixture/expected states and theme colors are independently fixed in code_config_fixture.py, with a hash bound into every result step. Text and bytes are never normalized to obtain a pass.

## Candidate and retained failures

Final run: a36b5761-8fc4-4073-90b8-7b48af1b2fcf. Binary SHA-256: 77b8b683349b7627e070af9e5daafc1e105cae9ba94a9f97eb3f822e897c73ab. Environment: Windows 10.0.26200, software renderer, dark private profile, keyboard/native-command automation and observed 96-DPI window. Build source manifest: 31f30207ddd40cb515227acd8eacde85afac82c5fcbcdedd021d025e55349796 at HEAD e6c1486ff0bb997d96801331b32986d612728d4f. Later synthetic-test/documentation edits do not relabel the build or native receipt; implementation and driver source hashes are retained separately.

All four failed native attempts used the prior binary c3b618c1dadce5ecd3ed023357bf48aa35d4585f3b057ef8a5e9ea502cd02a4f. The first found delayed initial provider publication; the next three exposed absent token geometry, including a retained screenshot of the visibly styled but folded document. Failed steps remain FAIL and dependent steps NOT_RUN. The initial oracle test's synthetic light-theme background was too close to ordinary text; its failed receipt and the corrected test are retained separately. None of those failures is promoted to PASS or erased.

All receipts record stable source during execution; raw stdout/stderr and native artifact hashes were checked. A final process check found no remaining Bareline process. [verification-summary.json](verification-summary.json) lists each run and the final implementation/adapter source hashes. [retained-files.json](retained-files.json) maps 117 exact-byte retained files (799,810 bytes) to original paths and hashes, including receipts, generated scratch files, screenshots and inert source copies. Native references preserve original scratch paths; use that mapping to review retained copies.

## Remaining qualification

**Two of thirteen procedures are implemented; eleven remain. Forty-seven top-level backlog items remain open.** The next procedure is regex_transform. The geometry fix is recorded within QUAL-024, whose broader qualification remains open. Other themes/renderers, OS floors, DPI, split geometry, physical screen readers/IME, other languages/grammar and completion concurrency are not qualified by this representative Rust workflow. Only 100% DPI is currently supported by this pixel procedure; light has a source oracle but no native capture.

No full suite, aggregate native run, release build, reviewed AC mapping or acceptance import ran. The manifest's AC hints remain provisional. Independent review and final qualification remain pending.
