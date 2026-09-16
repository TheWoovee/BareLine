# DEV-003 — Code/config native procedure

**State:** implemented and focused native-verified. DEV-003 remains open: two procedures are implemented and eleven remain unimplemented; required environment qualification is incomplete.

## Scope

Follow the unchanged three-step [journey contract](../../../tests/e2e/journeys.json): open generated UTF-8 source and observe highlighting for the selected language; use completion and indentation while preserving surrounding text; save/reopen exact expected bytes. Reuse the existing native adapter, owned scratch/Jobs, bounded UIA observations, foreground/desktop guards and checked clean Exit.

Use a small Rust fixture with a Unicode comment, keyword, string, number, a unique completion candidate and a closing brace. Observe language status and actual rendered token colors using UIA text-range geometry and a capture of the owned client window. A language label alone cannot pass highlighting. Trigger completion with Ctrl+Space, observe the real candidate and selection, accept with Enter, and check exact text plus single Undo/Redo. Save through the native command, close and reopen through the native dialog.

Authority: [PR-021 journeys](../../blueprint/PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md), [PR-008 language/styling](../../blueprint/PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md), [PR-013 completion/indentation](../../blueprint/PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md), and FC-09. This is a representative Rust journey, not full fifteen-language, grammar, stale-provider, comment-command, or physical accessibility acceptance. Do not add an AC mapping from the manifest's provisional case hints.

## Verification and boundary

The first native captures exposed an integration defect: render_frame called update_accessibility while on_redraw had temporarily removed self.renderer. Both resident and split text geometry therefore lacked the renderer's layouts. Move publication after renderer restoration and verify real token rectangles/pixels on the new incremental debug candidate. This small app fix is necessary for the observed procedure. The fixture opens folded in the existing product; explicitly invoke the owned native Unfold All command before inspecting visible tokens.

Focused adapter/fixture tests must reject missing/wrong styling, absent completion/selection, changed surrounding text/bytes, unsupported cells and source/artifact drift. Retain actual pixels and observations; do not normalize text or substitute synthetic captures for native evidence. Stop on lost foreground, physical Escape or locked desktop. Run only this journey against the existing debug candidate where possible; no full suites, aggregate native run, release build or unrelated implementation.

Report implemented/verified steps, remaining procedures and the top-level backlog count separately. Preserve previous receipts and dirty workspace changes. Update the status/backlog after the bounded check.

## Result

- Native code_config passes s1/s2/s3 and checked editor Exit 0 on debug binary 77b8b683349b7627e070af9e5daafc1e105cae9ba94a9f97eb3f822e897c73ab. The observed cell is Windows 10.0.26200, software renderer, dark private profile, keyboard/native-command automation and 96 DPI. Four visible token probes have nonempty UIA rectangles and the expected rendered colors.
- Ctrl+Space exposes answer_value; Down selects it and Enter accepts it. Exact text verifies indentation, unchanged Unicode/surrounding source, and completion Undo/Redo. Save/Close/Open preserves the independently expected UTF-8/LF bytes and clean state.
- 32 focused Python tests pass (22 shared adapter, 10 code/config oracle). Ten synthetic bitmap checks verify pixel sampling, coordinate origins, overlap deduplication and rejection of invalid geometry. They are source tests, not native product evidence.
- One incremental debug app build passed in 7.403 seconds after moving accessibility publication out of render_frame to follow renderer restoration. The native driver also waits at most three seconds for initial Editor provider publication. No input is replayed by observation polling.
- [Retained evidence](../../qa/2026-09-15-code-config/README.md) preserves the intermediate synthetic test failure, all four failed pre-fix native attempts, and the final passing run. No full suite, aggregate journey run, release build, AC mapping or acceptance import ran.

The geometry sub-fix is recorded inside existing QUAL-024; its split/physical/platform qualification remains open. Code/config is currently executable at 100% DPI; only the dark/software cell has a native pass. Other languages, themes, renderer modes, OS floors, DPI and physical screen-reader/IME observations remain for final qualification.

**47 of 49 top-level backlog items remain open. Eleven DEV-003 procedures remain unimplemented.** Next: regex_transform. Existing new-file encoding UI cells also remain queued for final validation.
