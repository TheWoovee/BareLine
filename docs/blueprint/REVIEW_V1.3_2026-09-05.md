# Bareline v1.3 Revision Review

**Version:** 1.3 · **Date:** 2026-09-05

Complete 22 items; leave item 18 partial only for exact 28 px workspace row density. Preserve all images/documents and every implementation tracker state. Scores judge documentation or visual-reference usefulness, not working software. No prototype, application test or benchmark ran. Run documentation validation and vector asset rendering only.

## Items 1-23

Verify claims against current files before editing. Approximate review line numbers moved during revision; cite final locations below.

| Item | Status | Finding and disposition | Evidence |
|---|---|---|---|
| 1 | Done | Valid: AC-001-03 and downstream briefs consumed absent scope interfaces; add ownership, fake-only trait sketches and four likely paths. | [PR-001:26](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:26); [PR-001:80](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:80); [PR-001:71](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:71) |
| 2 | Done | Valid: the IME/DPI acceptance case exceeded the scaffold; retain AC-001-01 and add a bounded DirectWrite/BiDi/preedit/device-loss prototype. | [PR-001:29](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:29); [10_ACCEPTANCE_AND_TRACEABILITY.md:56](10_ACCEPTANCE_AND_TRACEABILITY.md:56); [scripts/revision_cases.json:8](scripts/revision_cases.json:8) |
| 3 | Done | Valid: view-cache wording contradicted FC-03; require owned inverse bytes before commit, using equivalent wording that also satisfies item 21. | [PR-002:98](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:98); [09_FOUNDATION_CONTRACTS.md:35](09_FOUNDATION_CONTRACTS.md:35) |
| 4 | Done | Valid: PR-002 scope and design did not identify the shared logical-actor pool; replace both statements with worker bounds and no owned OS thread. | [PR-002:36](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:36); [PR-002:84](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:84) |
| 5 | Done | Valid: principle 6 incorrectly removed installed runtime files on disable; distinguish zero host/IPC/memory cost from explicit removal. | [01_REQUIREMENTS.md:60](01_REQUIREMENTS.md:60) |
| 6 | Done | Valid: all 27 snapshots used untyped canonical offsets; replace each with TextOffset/RawOffset and update the implementation and research paragraphs. | [02_IMPLEMENTATION.md:247](02_IMPLEMENTATION.md:247); [00_RESEARCH_BASELINE.md:22](00_RESEARCH_BASELINE.md:22); per-PR snapshot links below |
| 7 | Done | Valid: both huge-log comparison rows assumed competitor failure; require measured completion or timeout. | [01_REQUIREMENTS.md:464](01_REQUIREMENTS.md:464); [01_REQUIREMENTS.md:465](01_REQUIREMENTS.md:465) |
| 8 | Done | Valid: EncodingState retained an obsolete escape counter; expose separate invalid span and byte counts. | [PR-007:68](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:68); [PR-007:58](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:58) |
| 9 | Done | Valid audit: repair codec, undo, scheduling and runtime wording; retain only tab/version/security pinning and ordinary licensing prose outside the restricted scope. | [scripts/blueprint_checks.py:17](scripts/blueprint_checks.py:17); retained-hit rationale below |
| 10 | Done | Valid: ADR-37 through ADR-46 were pointers; give each Problem, Decision and Consequences, including the requested replacement rationale. | [07_DECISION_LOG.md:276](07_DECISION_LOG.md:276); [07_DECISION_LOG.md:348](07_DECISION_LOG.md:348) |
| 11 | Done | Valid: every brief had the same delivery instruction; generate 27 distinct three-row tables from 81 per-slice exit/proof records. | [scripts/revision_cases.json:6](scripts/revision_cases.json:6); [scripts/render_delivery_slices.py:6](scripts/render_delivery_slices.py:6); per-PR delivery links below |
| 12 | Done | Valid: FC-02 numerical budgets lacked settings keys; add matching ten-row tables and consumer references with effective Settings values. | [02_IMPLEMENTATION.md:295](02_IMPLEMENTATION.md:295); [PR-002:65](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:65); [PR-005:54](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md:54); [PR-006:56](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md:56); [PR-007:56](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:56) |
| 13 | Done | Valid: the three foundational storage briefs understated staging cost; add two-to-three-times sizing and UTF-8 Resident-first exits. | [PR-002:55](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:55); [PR-004:58](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md:58); [PR-007:52](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:52) |
| 14 | Done | Valid: v1.2 reused generic scoring reasons and unexplained reductions; supersede it with file-specific judgments and explicit image defects. | [REVIEW_V1.2_2026-09-05.md:1](REVIEW_V1.2_2026-09-05.md:1); [mockups/README.md:15](mockups/README.md:15); [mockups/README.md:12](mockups/README.md:12) |
| 15 | Done | Valid: 08 lacked executable full screen prompts; restore both prefixes and all requested screens from the historical compositions, plus seven adapted executed briefs with exact archived inputs. | [08_IMAGEN_PROMPTS.md:35](08_IMAGEN_PROMPTS.md:35); [08_IMAGEN_PROMPTS.md:49](08_IMAGEN_PROMPTS.md:49); [08_IMAGEN_PROMPTS.md:224](08_IMAGEN_PROMPTS.md:224) |
| 16 | Done | Valid: reference chrome allowed visible generic controls; reinstate hidden default and permit a dedicated optional screen only. | [11_UI_INTERACTION_SPEC.md:11](11_UI_INTERACTION_SPEC.md:11); [08_IMAGEN_PROMPTS.md:303](08_IMAGEN_PROMPTS.md:303) |
| 17 | Done | Valid: title, tab and placeholder conventions drifted; add explicit shared rules to UI spec and full prefixes, with utility tabs and the standard six documents. | [11_UI_INTERACTION_SPEC.md:9](11_UI_INTERACTION_SPEC.md:9); [08_IMAGEN_PROMPTS.md:37](08_IMAGEN_PROMPTS.md:37); [08_IMAGEN_PROMPTS.md:43](08_IMAGEN_PROMPTS.md:43) |
| 18 | Partially | Valid visual findings: replace all eight requested screen/hero assets, retain merge and restore five-state compare; exact workspace 28 px row density remains unresolved after two density refinements. | [mockups/NOTES.md:36](mockups/NOTES.md:36); [mockups/GENERATION_LOG.json:128](mockups/GENERATION_LOG.json:128); image evidence rows below |
| 19 | Done | Valid: no vector/ICO deliverable existed in the established mockups/logo-brand directory; hand-author four SVGs and render nine PNG/ICO sizes, explicitly pending human 16 px approval. | [00_BRAND_AND_LOGO.md:107](00_BRAND_AND_LOGO.md:107); [scripts/render_icons.py:14](scripts/render_icons.py:14); [mockups/NOTES.md:46](mockups/NOTES.md:46) |
| 20 | Done | Valid verification request: preserve the already matching dependencies, Mermaid and eight-cell tracker; add full AC/FC, root-index and image-dimension checks. | [scripts/validate_blueprint.py:29](scripts/validate_blueprint.py:29); [scripts/validate_blueprint.py:54](scripts/validate_blueprint.py:54); [scripts/validate_blueprint.py:80](scripts/validate_blueprint.py:80); [scripts/validate_blueprint.py:122](scripts/validate_blueprint.py:122) |
| 21 | Done | Valid: active wording contained prohibited regressions; the final scoped regression scan returns zero hits. | [scripts/blueprint_checks.py:17](scripts/blueprint_checks.py:17); exact output below |
| 22 | Done | Valid: the validator lacked regression, prompt and settings-table guards; add failing checks and probe their failure paths with in-memory document mutations. | [scripts/validate_blueprint.py:126](scripts/validate_blueprint.py:126); [scripts/validate_blueprint.py:129](scripts/validate_blueprint.py:129); [scripts/validate_blueprint.py:133](scripts/validate_blueprint.py:133) |
| 23 | Done | Valid reporting request: record item dispositions, exact evidence, per-file/image reasons and remaining limits; preserve all 27 tracker rows as NOT_STARTED. | [REVIEW_V1.3_2026-09-05.md:1](REVIEW_V1.3_2026-09-05.md:1); [03_PR_TRACKER.md:7](03_PR_TRACKER.md:7) |

## File review

Use one reason per file; do not interpret repeated numerical ratings as common evidence. Preserve historical ratings in archived reviews.

| File | Score / 10 | File-specific reason | Evidence |
|---|---|---|---|
| README.md | 8.5 | The index now identifies v1.3 and distinguishes the five-state compare reference from directional merge affordances. | [README.md:70](README.md:70) |
| 00_BRAND_AND_LOGO.md | 8.5 | The chosen interior-line mark now has vector sources and a render command, while native 16 px human approval remains pending. | [00_BRAND_AND_LOGO.md:107](00_BRAND_AND_LOGO.md:107) |
| 00_RESEARCH_BASELINE.md | 8 | The baseline now describes a valid text view with raw provenance without changing its research claims. | [00_RESEARCH_BASELINE.md:22](00_RESEARCH_BASELINE.md:22) |
| 01_REQUIREMENTS.md | 8.5 | Runtime disable/remove semantics and both huge-log comparison rows now agree with FC-07 and FC-10. | [01_REQUIREMENTS.md:60](01_REQUIREMENTS.md:60) |
| 02_IMPLEMENTATION.md | 8.5 | Typed positions and ten concrete budget keys make the document/storage boundary actionable, but runtime integration remains unproved. | [02_IMPLEMENTATION.md:295](02_IMPLEMENTATION.md:295) |
| 03_PR_TRACKER.md | 9 | All 27 rows preserve five NOT_STARTED states under a matching eight-cell header and delimiter. | [03_PR_TRACKER.md:7](03_PR_TRACKER.md:7) |
| 04_PR_DEPENDENCIES_AND_PARALLELISM.md | 9 | The dependency table matches all brief dependency sets and all 98 Mermaid edges without a cycle. | [04_PR_DEPENDENCIES_AND_PARALLELISM.md:105](04_PR_DEPENDENCIES_AND_PARALLELISM.md:105) |
| 05_NOTEPADPP_PARITY_MATRIX.md | 8 | The parity matrix remains a feature-scope reference whose product-level parity evidence is still pending. | [05_NOTEPADPP_PARITY_MATRIX.md:1](05_NOTEPADPP_PARITY_MATRIX.md:1) |
| 06_OPEN_SOURCE_AND_DISTRIBUTION.md | 8.5 | The distribution plan retains unsigned-payload evidence separately from signed release artifacts and was not widened in this pass. | [06_OPEN_SOURCE_AND_DISTRIBUTION.md:1](06_OPEN_SOURCE_AND_DISTRIBUTION.md:1) |
| 07_DECISION_LOG.md | 9 | All ten foundation ADRs now explain the v1.1 failure, selected contract and replacement cost rather than pointing to a section. | [07_DECISION_LOG.md:276](07_DECISION_LOG.md:276) |
| 08_IMAGEN_PROMPTS.md | 8.5 | Both full prefixes and every requested screen are concrete again, although generative outputs still require state-by-state inspection. | [08_IMAGEN_PROMPTS.md:35](08_IMAGEN_PROMPTS.md:35) |
| 09_FOUNDATION_CONTRACTS.md | 9 | The ten authoritative contracts retain their decisions while two obsolete negative formulations are replaced with equivalent valid-text and owned-byte language. | [09_FOUNDATION_CONTRACTS.md:7](09_FOUNDATION_CONTRACTS.md:7) |
| 10_ACCEPTANCE_AND_TRACEABILITY.md | 8.5 | All 81 cases resolve from the briefs and the PR-001 shaping case now matches its bounded prototype scope. | [10_ACCEPTANCE_AND_TRACEABILITY.md:56](10_ACCEPTANCE_AND_TRACEABILITY.md:56) |
| 11_UI_INTERACTION_SPEC.md | 8.5 | Exact title text, tab placement and hidden reference chrome are explicit, with older retained raster deviations documented separately. | [11_UI_INTERACTION_SPEC.md:9](11_UI_INTERACTION_SPEC.md:9) |
| PRs/README.md | 8 | The brief index remains the entry point for selecting a PR without changing tracker workflow. | [PRs/README.md:1](PRs/README.md:1) |
| mockups/README.md | 8.5 | Every root mockup file now has a row and huge-log/compare-options demotions identify visible defects. | [mockups/README.md:7](mockups/README.md:7) |
| mockups/NOTES.md | 9 | Image-specific limits distinguish the unresolved workspace density from corrected states and reserve 16 px approval for a human. | [mockups/NOTES.md:36](mockups/NOTES.md:36) |
| scripts/revision_cases.json | 9 | All 27 records now carry three distinct exits with concrete proof scenarios instead of only a semicolon summary. | [scripts/revision_cases.json:6](scripts/revision_cases.json:6) |
| scripts/render_delivery_slices.py | 8.5 | Delivery tables are rendered directly from the case records, preventing manually divergent slice descriptions. | [scripts/render_delivery_slices.py:6](scripts/render_delivery_slices.py:6) |
| scripts/blueprint_checks.py | 8.5 | Shared scanners enforce exact restricted wording and settings defaults while avoiding self-matches in their own definitions. | [scripts/blueprint_checks.py:17](scripts/blueprint_checks.py:17) |
| scripts/validate_blueprint.py | 9 | The validator now fails on stale slice/case text, unknown references, missing assets, prompt regressions and incomplete hash coverage. | [scripts/validate_blueprint.py:63](scripts/validate_blueprint.py:63) |
| scripts/render_icons.py | 8.5 | The renderer assembles all nine independently rendered ICO frames and archives changed outputs, but human optical approval is intentionally separate. | [scripts/render_icons.py:32](scripts/render_icons.py:32) |
| MANIFEST.json | 9 | Version/date and exhaustive SHA-256 coverage bind the revised package, with explicit self/output digest exclusions. | [MANIFEST.json:1](MANIFEST.json:1) |
| mockups/GENERATION_LOG.json | 9 | Measured PNG dimensions, actual generators and retained prompt/asset supersessions now provide auditable provenance. | [mockups/GENERATION_LOG.json:1](mockups/GENERATION_LOG.json:1) |
| PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md | 9 | The scaffold explicitly owns fake-only trust/codec/package interfaces and the one-line Windows text prototype required by its consumers. | [PR-001:16](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:16); [PR-001:274](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md:274) |
| PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md | 8.5 | Resident-first delivery now budgets side tables and disk stores, while owned inverse bytes and pool scheduling close the two blocking prose gaps. | [PR-002:16](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:16); [PR-002:206](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md:206) |
| PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md | 8.5 | Virtual painting, committed input and long-line cancellation have separate exits tied to grapheme and opaque-span boundaries. | [PR-003:16](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md:16); [PR-003:155](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md:155) |
| PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md | 8.5 | UTF-8 Resident lifecycle precedes durable baseline work and Paged/codec integration, making the larger recovery scope visible. | [PR-004:16](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md:16); [PR-004:173](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md:173) |
| PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md | 8.5 | Literal search, bounded regex completeness and linked dirty-document replacement now have independent exit proofs and shared budget keys. | [PR-005:16](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md:16); [PR-005:152](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md:152) |
| PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md | 8.5 | Column editing, in-memory clipboard limits and absent comment-provider behavior now close as separate deliverable slices. | [PR-006:16](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md:16); [PR-006:175](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md:175) |
| PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md | 8.5 | Separate invalid span/byte counters and staged UTF-8, legacy and disk-transcode paths make provenance cost explicit. | [PR-007:16](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:16); [PR-007:149](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md:149) |
| PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md | 8.5 | The fifteen-language catalog, provisional checkpoint highlighting and safe UDL replacement each have a distinct exit condition. | [PR-008:16](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md:16); [PR-008:146](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md:146) |
| PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md | 8.5 | Discovery cycles, revision-matched outlines and unknown-index minimaps are separated into independently reviewable behaviors. | [PR-009:16](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md:16); [PR-009:134](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md:134) |
| PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md | 8.5 | Tab persistence, unnumbered alignment spacers and clone state each receive an explicit proof scenario. | [PR-010:16](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md:16); [PR-010:129](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md:129) |
| PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md | 8.5 | Registry projections, Escape layering and AltGr/remapping are now separable, reducing ambiguity between command dispatch and text input. | [PR-011:16](PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md:16); [PR-011:132](PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md:132) |
| PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md | 8.5 | Scope security, theme persistence and point-unit DPI behavior now have different slice exits instead of one broad settings increment. | [PR-012:16](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md:16); [PR-012:134](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md:134) |
| PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md | 8.5 | Stale completion rejection, comment registration and bounded contextual typing each specify a distinct integration result. | [PR-013:16](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md:16); [PR-013:129](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md:129) |
| PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md | 8.5 | Macro failure location, direct process arguments and bounded output cleanup now have separate completion criteria. | [PR-014:16](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md:16); [PR-014:130](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md:130) |
| PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md | 8.5 | Watcher identity, partial-page tail continuity and real remote trust evaluation now have explicit staged ownership. | [PR-015:16](PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md:16); [PR-015:132](PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md:132) |
| PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md | 8.5 | Offline verified install can complete before online retrieval, while disable/remove lifecycle and host quotas retain separate proof cases. | [PR-016:16](PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md:16); [PR-016:160](PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md:160) |
| PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md | 8.5 | Dirty-target hunk application, cancellation semantics and export/printer failure handling no longer share one vague slice. | [PR-017:16](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md:16); [PR-017:162](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md:162) |
| PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md | 8.5 | Unsigned/signing verification, authenticated downloads and Windows path/portable integration now expose independent exits. | [PR-018:16](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md:16); [PR-018:149](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md:149) |
| PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md | 9 | Paired milestone methodology, populated-tab footprints and raw timeout reporting now define separate harness deliverables without claiming performance results. | [PR-019:16](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md:16); [PR-019:155](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md:155) |
| PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md | 8.5 | Recovery receipt faults, trust attacks and diagnostic redaction now have independent security review boundaries. | [PR-020:16](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md:16); [PR-020:157](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md:157) |
| PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md | 8.5 | Case-to-commit evidence, Windows assistive workflows and honest release readiness are now explicit separate QA deliverables. | [PR-021:16](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md:16); [PR-021:140](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md:140) |
| PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md | 8.5 | Neutral compilation, lossless platform paths and skeleton readiness remain distinct so headless results cannot imply a completed port. | [PR-022:16](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md:16); [PR-022:131](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md:131) |
| PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md | 8.5 | Keyboard primitives, cancellable text controls and DPI geometry now have concrete exits tied to the owning UI layer. | [PR-023:16](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md:16); [PR-023:153](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md:153) |
| PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md | 8.5 | Semantic tree coverage, bounded multi-GB text ranges and real high-contrast rendering each require their own evidence. | [PR-024:16](PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md:16); [PR-024:137](PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md:137) |
| PRs/PR-025_DIFF_CORE_ENGINE.md | 8.5 | Diff caps, collision-safe normalized equality and original-range application now close separately under explicit terminal states. | [PR-025:16](PRs/PR-025_DIFF_CORE_ENGINE.md:16); [PR-025:159](PRs/PR-025_DIFF_CORE_ENGINE.md:159) |
| PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md | 8.5 | Reviewed fingerprints, unique backup staging and durable receipt reconciliation are independently scoped for workspace replacement. | [PR-026:16](PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md:16); [PR-026:139](PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md:139) |
| PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md | 8.5 | JSON number fidelity, XML external-resource rejection and Hex generation labels now have separate extension-specific exits. | [PR-027:16](PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md:16); [PR-027:146](PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md:146) |

## Per-image review

Inspect the saved outputs visually. Use the generation-log line for binary provenance and the prompt line for the exact request. Exclude archived attempts from current scores.

| Image | Score / 10 | Specific reason | Evidence |
|---|---|---|---|
| [mockups/bareline-compare-merge.png](mockups/bareline-compare-merge.png) | 8.5 | Directional copy controls and the dirty target make this useful as the merge-affordance reference; use compare-dark for the five semantic states. | [mockups/GENERATION_LOG.json:101](mockups/GENERATION_LOG.json:101); [mockups/superseded/bareline-compare-dark-v1.2-v1.txt:1](mockups/superseded/bareline-compare-dark-v1.2-v1.txt:1) |
| [mockups/bareline-settings.png](mockups/bareline-settings.png) | 9 | Every setting row now shows its key, scope and pt units remain visible, and Reset section plus six neutral footer groups resolve the Settings-specific defects. | [mockups/GENERATION_LOG.json:128](mockups/GENERATION_LOG.json:128); [mockups/prompts/bareline-settings-v1.3.txt:1](mockups/prompts/bareline-settings-v1.3.txt:1) |
| [mockups/bareline-workspace-split.png](mockups/bareline-workspace-split.png) | 8.5 | Both Rust panes now show over 30 numbered lines, their own labels and divider sync control, but generated tree spacing still does not establish an exact 28 px layout. | [mockups/GENERATION_LOG.json:157](mockups/GENERATION_LOG.json:157); [mockups/prompts/bareline-workspace-split-v1.3.txt:1](mockups/prompts/bareline-workspace-split-v1.3.txt:1) |
| [mockups/bareline-extensions.png](mockups/bareline-extensions.png) | 9 | Installed/Discover/Updates/Disabled, the isolation banner, runtime removal and per-extension Permissions are visible with Bareline project publishers and readable capabilities. | [mockups/GENERATION_LOG.json:173](mockups/GENERATION_LOG.json:173); [mockups/prompts/bareline-extensions-v1.3.txt:1](mockups/prompts/bareline-extensions-v1.3.txt:1) |
| [mockups/logo-brand/bareline-hero.png](mockups/logo-brand/bareline-hero.png) | 9 | Windows controls, all twelve menus, the standard six tabs and sequential 1-24 lines replace the macOS chrome while leaving only the tagline outside the window. | [mockups/GENERATION_LOG.json:189](mockups/GENERATION_LOG.json:189); [mockups/prompts/bareline-hero-v1.3.txt:1](mockups/prompts/bareline-hero-v1.3.txt:1) |
| [mockups/bareline-recovery-center.png](mockups/bareline-recovery-center.png) | 8.5 | The title is Bareline and the unrelated greeting tab is replaced with notes.txt while Complete, Edits only and Corrupt tail actions remain legible. | [mockups/GENERATION_LOG.json:205](mockups/GENERATION_LOG.json:205); [mockups/prompts/bareline-recovery-center-v1.3.txt:1](mockups/prompts/bareline-recovery-center-v1.3.txt:1) |
| [mockups/bareline-source-changed.png](mockups/bareline-source-changed.png) | 8.5 | The command-icon row is removed while missing-region hatching, recovery protection time and export-gap messaging remain intact; title normalization is outside this requested edit. | [mockups/GENERATION_LOG.json:221](mockups/GENERATION_LOG.json:221); [mockups/prompts/bareline-source-changed-v1.3.txt:1](mockups/prompts/bareline-source-changed-v1.3.txt:1) |
| [mockups/bareline-replace-preview.png](mockups/bareline-replace-preview.png) | 9 | The command-icon row is removed while changed-since-preview exclusion and Apply to 2 unchanged files remain visible; the existing title is retained by the edit-only constraint. | [mockups/GENERATION_LOG.json:237](mockups/GENERATION_LOG.json:237); [mockups/prompts/bareline-replace-preview-v1.3.txt:1](mockups/prompts/bareline-replace-preview-v1.3.txt:1) |
| [mockups/bareline-compare-dark.png](mockups/bareline-compare-dark.png) | 8.5 | The restored reference shows all five diff states and the complete menu; compare counts and alignment remain illustrative rather than verified fixture evidence. | [mockups/GENERATION_LOG.json:266](mockups/GENERATION_LOG.json:266); [mockups/prompts/bareline-compare-dark-v1.3.txt:1](mockups/prompts/bareline-compare-dark-v1.3.txt:1) |
| [mockups/logo-brand/bareline-icon-16.png](mockups/logo-brand/bareline-icon-16.png) | 8 | The 16 px plain b retains an open bowl without fold, notch or text lines, but native-size human approval is still pending. | [mockups/GENERATION_LOG.json:697](mockups/GENERATION_LOG.json:697); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-20.png](mockups/logo-brand/bareline-icon-20.png) | 8 | The 20 px plain b keeps the simplified silhouette for fractional Windows scaling without adding crowded interior details. | [mockups/GENERATION_LOG.json:713](mockups/GENERATION_LOG.json:713); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-24.png](mockups/logo-brand/bareline-icon-24.png) | 7.5 | The 24 px mark includes the two interior lines and accent cut, although the small fold remains visually dense. | [mockups/GENERATION_LOG.json:750](mockups/GENERATION_LOG.json:750); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-32.png](mockups/logo-brand/bareline-icon-32.png) | 8 | The 32 px raster separates the two text bars while preserving the full-height stem and diagonal accent. | [mockups/GENERATION_LOG.json:780](mockups/GENERATION_LOG.json:780); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-40.png](mockups/logo-brand/bareline-icon-40.png) | 8 | The 40 px raster preserves the bowl opening and both text bars at the 250% small-icon scale. | [mockups/GENERATION_LOG.json:810](mockups/GENERATION_LOG.json:810); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-48.png](mockups/logo-brand/bareline-icon-48.png) | 8.5 | The 48 px raster shows the fold and cyan corner distinctly while retaining the plain left stem. | [mockups/GENERATION_LOG.json:840](mockups/GENERATION_LOG.json:840); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-64.png](mockups/logo-brand/bareline-icon-64.png) | 8.5 | The 64 px raster makes both interior lines readable with clean rounded tile edges. | [mockups/GENERATION_LOG.json:870](mockups/GENERATION_LOG.json:870); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-128.png](mockups/logo-brand/bareline-icon-128.png) | 9 | The 128 px raster clearly separates the folded corner, two text lines and accent cut on the dark tile. | [mockups/GENERATION_LOG.json:900](mockups/GENERATION_LOG.json:900); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/logo-brand/bareline-icon-256.png](mockups/logo-brand/bareline-icon-256.png) | 9 | The 256 px raster exposes the adopted two-line construction cleanly enough to inspect the stroke and corner geometry. | [mockups/GENERATION_LOG.json:930](mockups/GENERATION_LOG.json:930); [scripts/render_icons.py:1](scripts/render_icons.py:1) |
| [mockups/bareline-huge-log.png](mockups/bareline-huge-log.png) | 7.5 | Keep the reduction from 9: the mandatory menu bar is absent and 241 million long log lines are inconsistent with the displayed 3.8 GB fixture size. | [mockups/README.md:15](mockups/README.md:15) |
| [mockups/bareline-compare-options.png](mockups/bareline-compare-options.png) | 7 | Keep the reduction from 9: the window omits menu/status chrome and places competing whitespace checkboxes inside Colors instead of the FC-06 whitespace enum in General. | [mockups/README.md:12](mockups/README.md:12) |
| [mockups/bareline-main-editor-dark.png](mockups/bareline-main-editor-dark.png) | 8.5 | The dark text-first composition retains six editor tabs and a dominant code surface; use the current chrome spec for implementation. | [mockups/README.md:16](mockups/README.md:16) |
| [mockups/bareline-main-editor-light.png](mockups/bareline-main-editor-light.png) | 8 | The warm light editor provides the paired theme composition; apply the corrected functional contrast tokens when implementing it. | [mockups/README.md:17](mockups/README.md:17) |
| [mockups/bareline-find-bar.png](mockups/bareline-find-bar.png) | 8.5 | The docked inline find row preserves editing context and visible match navigation; error and incomplete states remain specified separately. | [mockups/README.md:14](mockups/README.md:14) |
| [mockups/bareline-search-panel.png](mockups/bareline-search-panel.png) | 8 | The grouped results panel demonstrates file hierarchy and docked search placement; displayed counts remain illustrative. | [mockups/README.md:20](mockups/README.md:20) |
| [mockups/bareline-command-palette.png](mockups/bareline-command-palette.png) | 8 | The palette shows command titles, menu paths and shortcut hints; disabled and no-match behavior still requires the interaction specification. | [mockups/README.md:9](mockups/README.md:9) |
| [mockups/bareline-tail-mode.png](mockups/bareline-tail-mode.png) | 8 | The follow banner distinguishes scroll pause and unlock actions; source-generation continuity remains a contract to implement. | [mockups/README.md:23](mockups/README.md:23) |

## Keep unchanged and reconcile conflicting instructions

- Keep the 04 dependency table, Mermaid and all tracker cells: the existing 27 dependency sets and 98 edges already agree; no decision change is justified.
- Keep AC-001-01 in PR-001 and AC-001-03 intact: the newly owned bounded prototype and interfaces satisfy the requested scope alternative; no AC ID migration is needed.
- Keep tab pinning, pinned package/toolchain versions and pinned signing roots: these hits describe ordering or trust, never an undo retention strategy. The ordinary “Forks are welcome” line in 06 is outside the explicit 01/11/prompt greeting restriction.
- Resolve item 3 versus item 21 by writing “view-cache retention is never an undo strategy”; its requested exact replacement otherwise contains a prohibited pinned/undo hit. FC-03 behavior stays identical.
- Record the historical surrogate escape rejection here: U+FFFD plus a side table preserves valid UTF-8 and distinguishes real replacement/private-use characters; ADR-37 uses equivalent terminology so active contract prose also satisfies the regression rule.
- Preserve exact v1.2 executed prompts in the archive instead of altering historical tool inputs; 08 labels the seven current-contract adaptations separately. The original correction brief is preserved verbatim with an active pointer, so quoted rejected wording cannot masquerade as current requirements.
- Keep source-changed and replace-preview titles and icons while removing only the command row, as item 18 explicitly directs; item 17 supplies the policy for future compositions. Keep retained screens outside the requested regeneration list and explain their limits rather than silently broadening scope.
- Use mockups/logo-brand, the existing asset directory; no root logo-brand directory existed. Keep ICO status DRAFT until a human views and approves the native 16 px result.
- Keep image scores below perfect: workspace row density remains unresolved, compare sample alignment/counts are illustrative, and images do not verify runtime interaction.

Keep historical "revised v1.2" ADR annotations and v1.2 subsection labels where they identify when unchanged decisions landed; each requested document header is version 1.3 dated 2026-09-05.

## Regression grep output

Command: `python scripts/blueprint_checks.py`

```text
Regression grep: 0 hits
Scope: package text; exclude superseded archives and REVIEW files; apply greeting restriction to requirements, UI spec and prompts.
```

The package excludes generated graft index/cache directories from manifest and content checks. Scan all package text outside superseded and REVIEW files, with the requested narrower greeting scope. Check undo-context pinning in either order on the same source line. The prompt-specific check also rejects any spelling/case of the forbidden command-strip term in active reference prompts.

Documentation guard probes: 11 rejected wording fixtures, 2 allowed-context fixtures, missing budget row detection and current-prompt check passed. These are checks of documentation validators, not application acceptance execution.

## Validator output

Command: `python scripts/validate_blueprint.py`

```json
{
  "version": "1.3",
  "date": "2026-09-05",
  "passed": true,
  "checks": {
    "local_links_checked": 613,
    "dependency_rows": 27,
    "dependency_edges": 98,
    "unique_acceptance_cases": 81,
    "foundation_contracts": 10,
    "unique_delivery_tables": 27,
    "requirements_mapped": 32,
    "tracker_header_cells": 8,
    "tracker_rows_not_started": 27,
    "architecture_decisions": 46,
    "functional_contrast_pairs": 48,
    "minimum_functional_contrast": 4.51,
    "generated_assets": 44,
    "superseded_replacements": 94,
    "mockup_root_files_indexed": 20,
    "regression_hits": 0,
    "reference_prompt_violations": 0,
    "settings_table_keys_each": 10,
    "manifest_files": 200,
    "verified_file_hashes": 198
  },
  "errors": [],
  "scope": "Documentation/index/asset validation only. No application tests or benchmarks."
}
```

Run only documentation/index/asset checks. Keep all application acceptance evidence and tracker states NOT_STARTED. Do not infer that Rust sketches compiled, Windows shaping ran, recovery fault scenarios executed or comparative benchmarks ran.
