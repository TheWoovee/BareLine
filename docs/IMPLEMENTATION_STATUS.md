# Implementation status

Updated 2026-09-08. The original blueprint is preserved under `blueprint/`; its NOT_STARTED tables describe the supplied specification. This file tracks the implementation.

Latest [batch10 closure](implementation/INTEGRATION-20260908-BATCH10.md) completes the remaining seven implementation packages after bounded full-scope reviews and passing automated gates. All 27 packages are IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE. Recorded cumulative coverage is 492 distinct Rust tests, and the default native build passes. Manual interaction, controlled performance, foreign-platform, signing and release acceptance remain pending; none is inferred from source completion. Earlier integration notes remain historical evidence.

| Package | State | Evidence |
|---|---|---|
| PR-001 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Foundation closure](implementation/PR-001.md): startup/frame/font consumers, optional compile-off spans, renderer contract and exact seven-baseline workflow delivered; batch03 tests/default build/scoped Clippy pass. Physical/foreign-host/performance acceptance pending. |
| PR-002 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Document engine closure](implementation/PR-002.md): configurable memory/history policies, bounded paging/spill, provenance and durable source transactions; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-003 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Editor surface closure](implementation/PR-003.md): bounded shaped resident/paged navigation, input, scroll, zoom and selection consumers connected; full-scope review and batch09 editor42/native28 pass. Physical input, visual and large-file acceptance remains pending. |
| PR-004 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Whole lifecycle closure](implementation/PR-004.md): Save Copy/All, recovery/provenance/Center compare, closed views and recent-file persistence connected; batch03 app53/native10 pass. Manual crash/input/visual acceptance pending. |
| PR-005 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Search closure](implementation/PR-005-search.md): mixed full-source search, bounded partial regex/capture expansion and actual folder controls; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-006 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Power editing closure](implementation/PR-006.md): global selections, measured columns, captured typing/commands and durable cross-document transfer; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-007 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Encoding closure](implementation/PR-007.md): lossless codec lifecycle, metadata undo, exact guarded failure ranges and full-source paged EOL status connected; batch06 app82/file-io48/editor29/native23 pass. Manual large-file, visual and detection-quality acceptance remains pending. |
| PR-008 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Syntax closure](implementation/PR-008.md): mapped source styling/folds, edit anchors and per-view persisted language selection; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-009 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Workspace closure](implementation/PR-009.md): lazy explorer, document list, outline import/export, bounded map and actual semantic consumers delivered; batch04 app70/native14/syntax18/backend40 pass. Large-fixture, upstream-definition and native acceptance pending. |
| PR-010 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Views closure](implementation/PR-010.md): per-pane styling, global scroll/folds and canonical off-viewport selection/session state connected; independent review and batch08 native23/paged2/workspace13/accessibility6 pass. Manual native input/visual/large-file acceptance remains pending. |
| PR-011 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Whole command UI closure](implementation/PR-011.md): native menus/palette/mapper/toolbar/context projections and atomic keymap persistence delivered; combined136 tests, scoped Clippy and native build pass. Physical input/scaling/visual and foreign-host acceptance pending. |
| PR-012 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Settings closure](implementation/PR-012.md): validated layered persistence, typed editor, theme/font consumers, localization and native menu/context refresh delivered; batch04 app70/settings16/UI21 pass. Native input, mixed-DPI and visual acceptance pending. |
| PR-013 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Completion closure](implementation/PR-013.md): verified per-pane completion/signature context, stable targets and captured smart typing; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-014 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Macro/process closure](implementation/PR-014.md): manager/persistence, ordered acknowledged input/power/search replay, workspace interpolation, output links and accessibility connected; mixed persistence/replay regression and batch03 pass. Manual acceptance pending. |
| PR-015 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Watch and trust closure](implementation/PR-015.md): actual pane follow/reload controls, source-change state and scoped remote-read authorization; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-016 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Extension platform closure](implementation/PR-016.md): verified manager, bounded captured readers, authenticated owned host lifecycle and actual four-component gate pass. Native manager/input/visual acceptance and production trust distribution remain pending. |
| PR-017 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Compare/utilities closure](implementation/PR-017.md): full-source comparison, streamed durable merge, export and print consumers connected; full-scope review and batch09 native28/app84/diff23 pass. Native printing, visual and manual acceptance remains pending. |
| PR-018 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Windows distribution closure](implementation/PR-018.md): CLI/IPC, portable roots, shell/tray, reviewed migration, authenticated update/runtime provider and packaging source integrated; batch04 native14/backend40/distribution7 and default build pass. Signed deployment, packaging execution and clean-VM acceptance pending. |
| PR-019 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Benchmark tooling closure](implementation/PR-019.md): full native scenario scheduling, provenance and bounded owned-disk/acquisition accounting delivered; independent source review and thirteen pure tests pass. Actual controlled native benchmarks remain pending. |
| PR-022 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Portability closure](implementation/PR-022.md): adapter skeletons, lossless path/modifier contracts, RecordingBackend probes, derived architecture guard and three-OS CI source complete; guard and negative fixtures pass at 021d8cb. Foreign-host execution and probe acceptance remain pending; no native product port claimed. |
| PR-023 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Toolkit closure](implementation/PR-023.md): full source audit found no required toolkit gap; batch03 shared controls/focus/theme/virtual-tree tests pass. Native composition, physical input and visual acceptance remain pending. |
| PR-024 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Accessibility closure](implementation/PR-024.md): bounded native providers, actual controller hierarchy/focus/actions and independently reviewed 97-scenario JSON baseline pass. Physical assistive-technology, IME and mixed-DPI acceptance remain pending. |
| PR-025 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Bounded diff core closure](implementation/PR-025.md): AC-025-01/02/03 passed locally; independent source review found no blocking core gap. Cross-platform compile remains pending; no full acceptance claim. |
| PR-026 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Replacement closure](implementation/PR-026.md): reviewed per-file outcomes, bounded capture expansion, durable rollback and 100-document atomic grouping; known source gaps reviewed as resolved and batch10 passes. Manual/external acceptance remains pending. |
| PR-020 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Hardening closure](implementation/PR-020.md): bounded per-entry session salvage, actual owned process-death/storage-full fault tests, authenticated parser corpus and unsafe/sanitizer records delivered; independent review and batch07 gates pass. Manual/external acceptance remains pending. |
| PR-021 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Readiness tooling closure](implementation/PR-021.md): command inventory/journey tooling, complete final-asset verification and host/SDK license consumers delivered; independent source review and synthetic packaging tests pass. Actual journeys, signing and release acceptance remain pending. |
| PR-027 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [First-party extension closure](implementation/PR-027.md): JSON/XML/Hex source and all four serial actual release-host tests pass, including unchanged 1 GiB JSON and 5 GiB Hex fixtures. Integrated native/manual and signed-release acceptance remain pending. |

Phase count: 27 specified packages; 0 in progress, 27 implementation complete pending acceptance, 0 not started, 0 fully accepted. Assignment means work has started, not that a compiling or verified slice has been delivered. Delivered slices below are usable implementation, not whole-phase completion or a percentage of product readiness.

Owner requirements: work locally; match current mocks; start each PR with its own document; limit reading to necessary references; avoid redundant tests and repeated full builds.

## Whole-package closure

PR-025 is implementation complete with local AC-025-01/02/03 passing and an independent source review finding no blocking core gap; its required cross-platform compile remains pending. No package is fully accepted. `IN_PROGRESS` means source, integration or required local verification remains. `IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE` means required source and local verification are complete, with specifically identified external/cross-platform/native verification or acceptance outstanding; it is not equivalent to accepted. Focused module test counts alone do not earn that status.

| Closure candidate | Source versus acceptance boundary | Required next closure action |
|---|---|---|
| PR-022 | Required portable contracts, skeletons, architecture guard and CI source are complete; local checks pass. | Run foreign Linux/macOS contracts and native event-loop probes; no native product port is claimed. |
| PR-023 | Full owning and independent source audits found no remaining required toolkit gap; shared focus/theme/virtual-tree tests pass in batch03 and batch04. | Verify composed native controls, physical IME/AltGr/dead keys, pointer/focus, DPI and screenshot equality when manual acceptance resumes. |
| PR-025 | Complete neutral core source and local AC-025-01/02/03 evidence reviewed; 23 diff tests plus shared-fold test, explicit full 2 GiB-per-side traversal, active cancellation, collision/apply properties, lint and recorded 10 MiB/1 GiB performance support closure. Independent reviewer inspected source/evidence without rerunning the suite. | Execute the required cross-platform compile and record its result. Native UI is explicitly outside PR-025. No blocking source gap or additional owner-permission gate was identified; full acceptance is not yet claimed. |

Prioritize these concrete closure actions over treating additional module assignments as completed phases.

## Active ownership

| Lane | Exclusive implementation ownership | Current delivery boundary |
|---|---|---|
| integration | `apps/bareline/**`, `crates/app/**`, `crates/editor-surface/**`, `xtask/**` except explicitly delegated new modules below; shared dependency wiring | Integrate search/replacement, streaming lifecycle and feature consumers; coordinate shared APIs and the proportional integration compile. Syntax ownership released to power_edit/completion. |
| commands / macros | `crates/commands/**`, new `crates/app/src/palette.rs`, `crates/macros/**`, assigned platform process adapter, PR-011/014 notes | Verified command core; palette then macro/process work. Shared application module registration, routing and native menu wiring remain integration-owned. |
| search | `crates/search/**`, assigned PR-026 workspace-replacement additions, PR-005/026 evidence | Search engine then workspace replacement; application controls and routing go through integration. |
| file_core | `crates/document/**`, `crates/file-io/**` except codec/session/tail-owned files, PR-002/004 evidence | Paged shared-tree edit checkpoint passed; sparse indexing next. Owns module exports and lifecycle consumption. |
| codecs | New `crates/file-io/src/codecs/**`, PR-007 evidence | Streaming codecs against existing sink contracts. Coordinate module registration with file_core; do not modify lifecycle independently. |
| session | New `crates/file-io/src/session/**`, `docs/implementation/PR-004-session.md`; recovery follow-on files coordinated with file_core; new `crates/app/src/session_ui.rs` after journal work | Versioned bounded session persistence and recovery follow-on using the coordinated portability path API. Module registration and startup/lifecycle wiring go through file_core and integration. |
| diff / compare | `crates/diff/**`, assigned compare/utilities modules, PR-025/017 evidence | Diff checkpoint verified; compare/utilities follow-on active, with shared app wiring coordinated centrally. |
| portability / settings | `crates/platform/**` except assigned watcher/process/accessibility additions, `crates/platform-linux/**`, `crates/platform-macos/**`, `.github/workflows/**`, `crates/settings/**`, PR-022/012 evidence; new `crates/app/src/settings.rs` | Portability source slice locally verified; settings core compiles, UI next. Shared app registration stays with integration. CI configuration is not cross-OS execution evidence. |
| controls / views | `crates/ui/**`, assigned new `views.rs`, PR-023/010 evidence | Controls checkpoint verified; split-view follow-on active. Preserve existing consumers and coordinate shared app/editor registration. |
| power_edit / completion | Assigned `power_edit` and completion modules, `crates/syntax/**`, PR-006/008/013 notes | Power-edit checkpoint verified; completion/language follow-on authorized. Shared editor registration stays with integration. |
| workspace | New workspace crate and assigned controller, PR-009 note | Workspace panels/model/controller implementation; shared application routing coordinated centrally. |
| extensions | Assigned extension runtime, `crates/extensions-protocol/**`, assigned native transport, PR-016 note | Isolated runtime/protocol/transport implementation; current work does not establish full isolation or package acceptance. |
| watch_trust | Assigned `path_trust`/`watch` modules, file-io `tail` module, platform watch adapter, PR-015 note | Conservative path classification, bounded watch/tail processing; coordinate shared exports and lifecycle consumption. |
| accessibility | Assigned neutral/native/app accessibility modules, PR-024 note | AccessKit semantic/UIA bridge; shared UI semantic producers and app wiring require owning-lane coordination. |
| packaging | New `crates/distribution/**`, PR-018 note | Local distribution/packaging core; no publication or signed release claim. |
| performance | New xtask benchmark module, PR-019 note | Reproducible workload tooling; xtask entry-point integration stays central and performance results are not feature acceptance. |

Shared manifest/lockfile changes are coordinated centrally. Each lane starts or updates its own implementation note and records actual focused checks before its slice enters the delivered list. This tracker is maintained centrally; the preserved blueprint tracker is not rewritten.

## Package dependency queue

Dependencies below identify required stable interfaces, not completed or accepted packages. A narrow compiling slice may proceed once its consumed interfaces are stable; it does not establish whole-package completion.

| Package / feature | Current owner or queue | Direct prerequisites | Next implementation boundary |
|---|---|---|---|
| 001 Foundation/shell | integration | None | Close shell/command/render gaps and record native acceptance later. |
| 002 Document/undo | file_core | 001 | Source-backed Paged roots now active; bounded indexing/spill, quotas and source changes require final implementation evidence. |
| 003 Editor/input | integration | 001, 002, 023 | Complete navigation/selection/viewport behaviors against document interfaces. |
| 004 Lifecycle/session/recovery | file_core + session + integration | 002, 003, 023, 025 | Streaming Resident lifecycle/save safety and bounded sessions; recovery follow-on active, diff and startup consumption require integration. |
| 005 Search/replace | search + integration | 002, 003, 023 | Finish integrated replacement; then regex, paged/folder search and results. |
| 006 Power editing | power_edit + integration | 003, 023 | Assigned power-edit module; multi-cursor/rectangle/line/bookmark integration follows stable interfaces. |
| 007 Encoding/EOL | codecs + file_core | 002, 004 | Streaming fidelity, provenance/BOM/EOL; then paged transcode quotas and lifecycle integration. |
| 008 Syntax/folding/UDL | power_edit/completion + integration | 002, 003, 007 | Complete language/folding coverage with codec integration; shared app consumers stay central. |
| 009 Workspace panels | workspace | 003, 008, 023 | Workspace crate/controller active; explorer/list/outline/map consume stable controls and syntax. |
| 010 Views/tabs | controls/views | 003, 004, 023 | Views module active after controls checkpoint; split/synchronized scrolling integration pending. |
| 011 Commands/palette/keymaps | commands + integration | 001, 003, 023 | Complete real palette, keymaps and menu/context registrations. |
| 012 Settings/themes/localization | portability/settings + integration | 001, 003, 011, 023 | Settings core active; expand renderer-only configuration through coordinated command and semantic-token interfaces. |
| 013 Completion/smart typing | power_edit/completion | 003, 006, 008, 023 | Language-aware editing and CommentProvider follow-on active after power-edit checkpoint. |
| 014 Macros/external commands | commands/macros | 006, 011, 023 | Neutral recorder/process/output backend after palette; platform process adapter and application consumption follow. |
| 015 Watching/tail/path trust | watch_trust | 004, 007 | Handle-based trust, watching/tail and safe remote-path lifecycle active; grants and native behavior remain pending. |
| 016 Extension platform | extensions | 001, 011, 012 | Runtime/protocol/native transport active; verified packages and isolated-host acceptance remain pending. |
| 017 Utilities/compare/export/print | diff/compare | 003, 007, 010, 011, 012, 023, 025 | Compare/utilities active after diff checkpoint; consume coordinated split/theme interfaces. |
| 018 Windows packaging/CLI/updater | packaging | 004, 011, 012, 015 | Local distribution core/helper and synthetic packaging verified in PR-018; fresh integrated unsigned debug portable checkpoint byte-verified. Production signing, installer tooling and clean-VM acceptance unavailable; no publication authorized. |
| 019 Huge-file benchmarks | performance | 002, 003, 005, 008, 009, 015 | Benchmark tooling active; final workloads require real feature paths. |
| 020 Security/recovery hardening | Queued | 004, 015, 016, 018, 026 | Harden implemented paths; earlier lanes retain immediate safety obligations. |
| 021 Windows release QA | Queued | 004–020, 022–027 | Integrated parity and release evidence after implementation; no readiness claim now. |
| 022 Portability | portability | 001 | Lossless paths and architecture checks; adapters/CI then actual OS evidence. |
| 023 UI controls | controls | 001 | Complete reusable controls/semantics and their existing consumer integration. |
| 024 Accessibility/UIA | accessibility | 003, 011, 023 | AccessKit neutral/native/app bridge active; native assistive-technology verification later. |
| 025 Diff | diff | 002 | Bounded neutral diff; hand off stable results to lifecycle and compare consumers. |
| 026 Workspace replacement | search | 004, 005, 007 | Assigned next slice: captured-state multi-file replacement with lifecycle safety and encoding fidelity. |
| 027 First-party extensions | Queued | 016 | JSON/XML/Hex through the actual isolated extension platform. |

After the current wave: integrate diff/codecs/controls/session/commands into existing consumers, implement source-backed Paged roots and bounded indexing, and extend session persistence into the recovery journal/receipts before claiming recovery. Advance power-editing slices through an explicit editor ownership handoff. Split views, settings and later language/workspace features follow their stable prerequisites. Extensions and full release hardening do not move ahead merely to fill a parallel lane.

The completed integration checkpoint passes `cargo check -p bareline --locked`, six app tests, two editor tests, one native WIC style check and focused Clippy. PR-005/008 notes contain the owning evidence. This verifies that checkpoint, not later streaming/regex changes still being integrated or whole-package acceptance.

### Incoming focused evidence — provisional

These are agent-reported checkpoints, not final reports for changes still in flight. Later edits require the owning lane's final scoped verification before promotion into delivered evidence.

| Lane | Reported checkpoint | Still open |
|---|---|---|
| search | 13 focused bounded-regex tests and lint passed. | Work continues; final search report, current consumer integration and subsequent-change verification. |
| codecs | 5 focused tests passed. | Final codec report, lifecycle consumption and provenance/fidelity integration. |
| diff | Latest checkpoint: 15 focused tests and source review passed. | Compare/utilities follow-on and consumer integration remain open. |
| file_core | Latest PagedDocument shared-tree edit checkpoint: 4 tests passed; reviewer reported no blockers. Earlier streaming checkpoint had 3 tests. | Sparse indexing next; viewport/lifecycle consumption and final source-backed paging evidence remain open. |
| controls | Latest checkpoint: 16 focused tests and lint passed. | Views follow-on and consumer integration remain open. |
| session | Recovery backend checkpoint: 9 tests passed. | Controller/UI consumption next; [recovery note](implementation/PR-004-recovery.md) still marks implementation in progress. |
| settings | Core compiles; [settings note](implementation/PR-012.md) records active schema/theme/localization work. | UI next; no final behavior or visual acceptance claimed. |
| power_edit | 7 focused tests and lint passed. | Completion/syntax follow-on and integrated editing acceptance remain open. |
| file-io integration | 36 scoped tests passed; reviewer verified recovery and paged-diff fixes. | Subsequent changes, controller consumption and whole lifecycle/recovery acceptance remain open. |
| performance | Generated 1 MiB smoke: first viewport reads 64 KiB / one page. | Further real-scenario harness and larger workloads remain pending; no general performance or huge-file acceptance claim. |

Source-backed paging and session recovery/journal follow-ons are active source work, not lock-only blockers or completed features.

Commands core final checkpoint: seven focused tests and Clippy passed. The follow-on `crates/app/src/palette.rs` module is active and requires its own integration evidence; the complete palette/keymap package is not accepted.

### Final local portability evidence

PR-022 reports 11 final focused cases, Clippy with warnings denied, targeted formatting, the architecture guard with negative fixtures, and both recording-probe example compiles passing on Windows. Lossless Windows native paths and both tagged identity decoders are covered. See the [owning evidence note](implementation/PR-022.md) for exact commands and limitations. Linux/macOS execution, Unix native path round trips and foreign-host shared UI execution remain pending; no native port or package acceptance is claimed.

Queued manual check after an unlocked desktop is available: run `cargo run -p bareline-platform-linux --example recording_probe_linux`, confirm the event-loop probe responds to input and closes. RecordingBackend emits operations and has no pixel painter, so this is not a visual-rendering comparison. Foreign-host execution requires Linux/macOS hosts independently. This queue entry does not close or replace the unlocked-PC checklist.

## Delivered slices

- Local Rust workspace and 202 byte-verified copied reference files; no remote or push.
- Native Direct2D/DXGI rendering, software fallback, shaped text and DPI geometry, command/menu foundation, renderer settings and launch/idle harness.
- Persistent Resident document tree, immutable snapshots, atomic edits, bounded actor scheduler and owned undo/redo.
- Resident multiline editing, grapheme movement, selection, composition overlay, clipboard hooks, scrolling, dirty tabs and individual tab close.
- Worker-backed UTF-8/BOM open up to 1 MiB, staged captured-state save with conflict checks, cooperative cancellation and original-file preservation checks.
- Worker-backed Unicode literal find, case folding, whole-word matching, navigation and a reusable IME-aware field. Backend Extended escapes and cancellable single/all-match replacement preparation have focused passing coverage. F3 retains completed results after closing Find. Extended-mode selection and replacement UI are being integrated; do not count them as accepted.
- Real client-area offscreen rendering at 100%, 150% and 200%; reviewed geometry corrections without creating a window. Full native mock fidelity remains unaccepted.

Normal launch creates an editable Untitled document; command-line file paths open after the first frame. Focused validation is recorded in the owning notes. Source changes in active parallel work are not considered verified until their integration checks are recorded.

PC locked: desktop automation stopped at owner request. Continue local source work and headless verification. Mock equality, physical IME interaction and interactive dialogs have not been visually accepted.

The ordered [unlocked-PC completion checklist](UNLOCK_CHECKLIST.md) tracks every deferred desktop check and the evidence needed to close it. Update it as new screens and interactions are implemented.

## Remaining implementation, in dependency order

1. Finish current search/replace integration and syntax slice, then run scoped behavioral checks and one integration compile. Compare affected client screens against their mocks. Full regex, paged/folder search, marks, result panels and linked replacement remain PR-005/026 work.
2. Integrate actual file-backed paging, bounded indexing and disk spill with the document tree; remove the current 1 MiB open limitation through the designed streaming path. Add quota/source-change handling and complete long-line navigation.
3. Complete file lifecycle: save barriers and existing-target Save As, read-only behavior, session persistence, recovery journal/receipts and conflict/diff integration. Cooperative file cancellation is already delivered; OS-call timeout policy remains open.
4. Complete editor/control behaviors: drag and multi/rectangle selection, power editing, wrap/horizontal scrolling, split views, workspace panels, command palette/keymaps, themes and accessibility.
5. Implement remaining encoding/language features, macros/utilities, monitoring, extension isolation, diff/workspace replacement, Windows packaging and hardening according to their own briefs.
6. Complete native visual/input acceptance when unlocked, then proportional release/performance/security evidence. Cross-platform readiness and remote CI evidence are still pending. Do not push or publish without owner authorization.

The lock blocks native desktop verification, not these implementation steps. Independent modules may proceed in parallel with explicit file ownership; integrate dependency changes centrally. Full release readiness is not claimed.
