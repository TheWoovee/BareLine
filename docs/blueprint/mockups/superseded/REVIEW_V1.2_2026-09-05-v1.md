# Bareline v1.2 Revision Review

**2026-09-05 · Overall blueprint: 8.5/10, previously 7/10.** These are reviewer judgments about documentation/design quality, not completion percentages. Working software, benchmark superiority and production accessibility remain unverified. The [original review](REVIEW_2026-09-05.md) is preserved unchanged.

## What changed

- Revised requirements and architecture around separate text/raw offsets, original-byte provenance, disk-backed transcodes, source generations, CR/LF/CRLF aggregates and owning undo.
- Added ten normative foundation contracts and ADR-37–46 covering recovery durability/availability, safe saves, tail continuity, bounded regex/search/diff, linked undo, extension quotas and release trust.
- Updated all 27 implementation briefs with smaller delivery increments and 81 explicit review acceptance cases. Corrected future dependency assumptions and added PR-013 → PR-006.
- Added UI state/keyboard/accessibility specifications, stronger light-theme focus/comment/gutter tokens, truthful parity status and a reproducible benchmark method.
- Created seven ChatGPT-generated image assets: five replacements plus unavailable-source and workspace-replace-preview states. Preserved replaced originals and obsolete boards under superseded.

## Area scores

| Area | Before | After | Remaining limitation |
|---|---:|---:|---|
| Requirements | 8 | 8.5 | Exhaustive command-level cases and product validation pending |
| Implementation architecture | 6 | 8 | High-risk storage/regex/render contracts require prototypes |
| PR briefs | 7 | 8.5 | No implementation or integration evidence yet |
| Current visual direction | 8 | 8.5 | Some retained screens still have chrome/content defects |
| UI handoff | 6 | 8 | Written states improved; interactive prototype absent |
| Brand concepts | 7.5 | 7.5 | PNG geometry unchanged; final vector/ICO remains pending |
| Working implementation | Not assessable | Not assessable | No application code/build in this package |

## Review comment disposition

| Original concern | Resolution in this revision | Evidence still needed |
|---|---|---|
| Invalid-byte model and codec coverage | FC-01/02; revised ADR-02, requirements, PR-002/007 | Codec fixtures and offset prototype |
| Paged undo, baseline recovery and durable ordering | FC-02/03; PR-002/004; UI-RC/SC | Crash, eviction, truncation and disk-full runs |
| Long lines, regex context and incomplete replace | FC-05; PR-005/026; UI-FN/RP | Supported PCRE2 catalog and boundary fixtures |
| Tail/source/save races and dirty undo | FC-04/06; PR-004/015 | Real filesystem and concurrency tests |
| Thread budgets and diff output/cancellation | FC-02/06; PR-002/025 | Populated-tab and divergent-file measurements |
| Hidden dependencies and huge PR scope | Early PR-001 contracts; staged integration; per-brief increments | Compile/integration evidence and assigned human owners |
| XML/Hex/IPC extension contracts | FC-07; PR-016/027 | Wasmtime quota, parser and transport tests |
| Signing order, offline keys and trust rollback | FC-08; revised distribution process | Provider enrollment and signing rehearsal |
| Contrast, units and missing safety states | Revised tokens, UI specification and seven images | Real renderer, assistive tech and usability verification |
| Parity, source dates and marketing claims | Revised research/parity and FC-10 | Baseline executable/hardware and actual measurements |
| Manifest and mockup provenance | Complete index, hashes and generation/supersession log | Keep metadata synchronized with future edits |

## Per-file reassessment of the original 70 assets

Unchanged images keep their previous scores. Archiving obsolete imagery improves package clarity; it does not improve the image itself. Scores for briefs assess specification quality only.

| File | Before | After | Revision / remaining work |
|---|---:|---:|---|
| [README.md](README.md) | 7.5 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [00_BRAND_AND_LOGO.md](00_BRAND_AND_LOGO.md) | 8 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [00_RESEARCH_BASELINE.md](00_RESEARCH_BASELINE.md) | 7 | 8 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [01_REQUIREMENTS.md](01_REQUIREMENTS.md) | 8 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [02_IMPLEMENTATION.md](02_IMPLEMENTATION.md) | 6 | 8 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [03_PR_TRACKER.md](03_PR_TRACKER.md) | 8 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [04_PR_DEPENDENCIES_AND_PARALLELISM.md](04_PR_DEPENDENCIES_AND_PARALLELISM.md) | 6.5 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [05_NOTEPADPP_PARITY_MATRIX.md](05_NOTEPADPP_PARITY_MATRIX.md) | 6.5 | 8 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [06_OPEN_SOURCE_AND_DISTRIBUTION.md](06_OPEN_SOURCE_AND_DISTRIBUTION.md) | 7 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [07_DECISION_LOG.md](07_DECISION_LOG.md) | 6.5 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [08_IMAGEN_PROMPTS.md](08_IMAGEN_PROMPTS.md) | 6 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [PRs/README.md](PRs/README.md) | 8 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [mockups/README.md](mockups/README.md) | 8 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [mockups/NOTES.md](mockups/NOTES.md) | 6 | 8.5 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [MANIFEST.json](MANIFEST.json) | 6.5 | 9 | Updated contract/index and linked evidence; runtime verification remains pending. |
| [PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md) | 5.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md) | 6 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md) | 6 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md) | 5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md) | 7 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md](PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md) | 8.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md](PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md) | 7 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md](PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md) | 6.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) | 7 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md) | 7.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md) | 8 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md](PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md) | 7 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-025_DIFF_CORE_ENGINE.md](PRs/PR-025_DIFF_CORE_ENGINE.md) | 7 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md](PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md) | 6.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md](PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md) | 6.5 | 8.5 | Updated foundation contract, delivery increments and three atomic review cases; implement and verify. |
| [mockups/bareline-main-editor-dark.png](mockups/bareline-main-editor-dark.png) | 8.5 | 8.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-main-editor-light.png](mockups/bareline-main-editor-light.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-workspace-split.png](mockups/bareline-workspace-split.png) | 7 | 8 | Correct Rust outline/content and full chrome; second pane needs its own visible config.rs tab and placeholder icon needs production replacement. |
| [mockups/bareline-find-bar.png](mockups/bareline-find-bar.png) | 8.5 | 8.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-search-panel.png](mockups/bareline-search-panel.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-settings.png](mockups/bareline-settings.png) | 7 | 8.5 | Point units, scope, saved-state feedback and full menu; generated title/tab arrangement and sample footer require implementation normalization. |
| [mockups/bareline-extensions.png](mockups/bareline-extensions.png) | 7 | 8.5 | Runtime removal, per-extension controls and byte-mode labels; generated publisher/version are placeholders and JSON panel chip is omitted. |
| [mockups/bareline-huge-log.png](mockups/bareline-huge-log.png) | 7.5 | 7.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-compare-dark.png](mockups/bareline-compare-dark.png) | 7 | 8.5 | Directional merge, complete menus, aligned four-line sample and unsaved target; all five diff states still require interactive verification. |
| [mockups/bareline-compare-options.png](mockups/bareline-compare-options.png) | 7 | 7 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-recovery-center.png](mockups/bareline-recovery-center.png) | 7.5 | 8.5 | Complete/edits-only/corrupt-tail states and separate recovered copy; general help should be distinguished from selected row status. |
| [mockups/bareline-command-palette.png](mockups/bareline-command-palette.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/bareline-tail-mode.png](mockups/bareline-tail-mode.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-logo-mark-light.png](mockups/logo-brand/bareline-logo-mark-light.png) | 7.5 | 7.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-logo-mark-dark.png](mockups/logo-brand/bareline-logo-mark-dark.png) | 7.5 | 7.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-logo-exploration.png](mockups/logo-brand/bareline-logo-exploration.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-app-icon.png](mockups/logo-brand/bareline-app-icon.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-app-icon-light.png](mockups/logo-brand/bareline-app-icon-light.png) | 7.5 | 7.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-wordmark.png](mockups/logo-brand/bareline-wordmark.png) | 8 | 8 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-wordmark-dark.png](mockups/logo-brand/bareline-wordmark-dark.png) | 7 | 7 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/logo-brand/bareline-hero.png](mockups/logo-brand/bareline-hero.png) | 5.5 | 5.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [All_Screens.png](All_Screens.png) | 4 | 4 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [Comparison.png](Comparison.png) | 5 | 5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/superseded/bareline-workspace-split-v1.png](mockups/superseded/bareline-workspace-split-v1.png) | 5.5 | 5.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/superseded/bareline-find-bar-v1.png](mockups/superseded/bareline-find-bar-v1.png) | 6.5 | 6.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/superseded/bareline-search-panel-v1.png](mockups/superseded/bareline-search-panel-v1.png) | 6.5 | 6.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/superseded/bareline-compare-dark-v1.png](mockups/superseded/bareline-compare-dark-v1.png) | 6 | 6 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |
| [mockups/superseded/bareline-command-palette-v1.png](mockups/superseded/bareline-command-palette-v1.png) | 6.5 | 6.5 | Unchanged pixels; retain original review corrections. Archive-only where marked obsolete. |

## New artifacts

| Artifact | Score / 10 | Remaining work |
|---|---:|---|
| [Foundation contracts](09_FOUNDATION_CONTRACTS.md) | 8.5 | Prototype the contracts; budget tuning is pending |
| [Acceptance and traceability](10_ACCEPTANCE_AND_TRACEABILITY.md) | 8.5 | 81 new cases plus existing scenarios; enumerate all implemented commands |
| [Interaction specification](11_UI_INTERACTION_SPEC.md) | 8.5 | Build interactive prototype and validate assistive workflows |
| [bareline-source-changed.png](mockups/bareline-source-changed.png) | 8.5 | Unavailable regions, journal timestamp and partial export; restore explanatory text beside the hatched-area ellipsis in implementation. |
| [bareline-replace-preview.png](mockups/bareline-replace-preview.png) | 9 | Changed-since-preview exclusion and explicit eligible-file count; footer/context values remain illustrative. |

## Verification and next work

Run scripts/validate_blueprint.py for links, all 27 dependency rows, DAG consistency, acceptance IDs, tracker status, functional token contrast, manifest hashes and image dimensions. Its [validation result](VALIDATION_V1.2.json) records actual checks. No Rust compilation, application tests, benchmark trials or usability tests were possible because this remains a blueprint.

The highest-value next implementation work is the PR-001 real Windows text/input prototype, followed by PR-002 byte/undo ownership and PR-005 regex context proofs. A complete editable UI and production logo system remain separate deliverables. Scores were not raised for unchanged imagery or unverified software behavior.
