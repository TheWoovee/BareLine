# Implementation status

Updated 2026-09-06. The original blueprint is preserved under `blueprint/`; its NOT_STARTED tables describe the supplied specification. This file tracks the implementation.

| Package | State | Evidence |
|---|---|---|
| PR-001 | IN_PROGRESS | [Foundation delivery note](implementation/PR-001.md) |
| PR-002 | IN_PROGRESS | [Resident core and bounded scheduler](implementation/PR-002.md) |
| PR-003 | IN_PROGRESS | [Resident editor surface](implementation/PR-003.md) |
| PR-004 | IN_PROGRESS | [UTF-8 lifecycle](implementation/PR-004.md) |
| PR-005 | IN_PROGRESS | [Unicode literal/Extended search and replacement preparation](implementation/PR-005.md); replacement UI integration active, not yet accepted. |
| PR-006 | IN_PROGRESS | Power-edit module lane assigned; implementation evidence pending. |
| PR-007 | IN_PROGRESS | Codec lane assigned; streaming implementation and lifecycle integration evidence pending. |
| PR-008 | IN_PROGRESS | Syntax-highlighting implementation active in parallel; evidence note and focused verification pending. |
| PR-009 | IN_PROGRESS | Workspace crate/controller assigned; implementation evidence pending. |
| PR-010 | IN_PROGRESS | Views module assigned to controls after the PR-023 checkpoint. |
| PR-011 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Whole command UI closure](implementation/PR-011.md): native menus/palette/mapper/toolbar/context projections and atomic keymap persistence delivered; combined136 tests, scoped Clippy and native build pass. Physical input/scaling/visual and foreign-host acceptance pending. |
| PR-012 | IN_PROGRESS | Settings core assigned to portability lane; settings behavior and integration evidence pending. |
| PR-013 | IN_PROGRESS | Completion/syntax follow-on assigned after the power-edit checkpoint. |
| PR-014 | IN_PROGRESS | [Macro/process backend](implementation/PR-014.md) active after palette work. |
| PR-015 | IN_PROGRESS | [Path trust/watch/tail](implementation/PR-015.md) backend active; native behavior and consumer integration pending. |
| PR-016 | IN_PROGRESS | Extension runtime/protocol/native transport assigned; isolation/package acceptance pending. |
| PR-017 | IN_PROGRESS | Compare/utilities assigned after the diff checkpoint. |
| PR-018 | IN_PROGRESS | [Distribution evidence](implementation/PR-018.md): CLI/portable roots, authenticated update worker/helper atomic apply and rollback, notices and deterministic local packaging verified. Actual fresh unsigned debug portable checkpoint packaged and byte-verified; app supervision/shell/tray integration and signed installer acceptance remain pending. |
| PR-019 | IN_PROGRESS | Benchmark tooling assigned; complete-feature workloads and release measurements remain pending. |
| PR-022 | IN_PROGRESS | [Portability evidence](implementation/PR-022.md): documented local source slices complete and verified; foreign-host execution and native GUI acceptance pending. This is local implementation completion, not accepted native ports. |
| PR-023 | IN_PROGRESS | [UI primitives](implementation/PR-023.md) |
| PR-024 | IN_PROGRESS | [Accessibility adapter](implementation/PR-024.md) active; native assistive-technology acceptance pending. |
| PR-025 | IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE | [Bounded diff core closure](implementation/PR-025.md): AC-025-01/02/03 passed locally; independent source review found no blocking core gap. Cross-platform compile remains pending; no full acceptance claim. |
| PR-026 | IN_PROGRESS | Workspace replacement assigned as search lane's next slice; integration evidence pending. |
| PR-020 | IN_PROGRESS | [Security/recovery source review](implementation/PR-020.md): concrete cache and close findings reviewed with owner regression evidence; whole hardening acceptance remains open. |
| PR-021 | IN_PROGRESS | [Readiness audit](implementation/PR-021.md) and [native QA evidence](implementation/PR-021-NATIVE-20260906.md); full release gates and acceptance remain open. |
| PR-027 | IN_PROGRESS | [First-party extension evidence](implementation/PR-027.md): component/formatter harness work active; full integration and acceptance remain open. |

Phase count: 27 specified packages; 25 in progress, 2 implementation complete pending acceptance, 0 not started, 0 fully accepted. Assignment means work has started, not that a compiling or verified slice has been delivered. Delivered slices below are usable implementation, not whole-phase completion or a percentage of product readiness.

Owner requirements: work locally; match current mocks; start each PR with its own document; limit reading to necessary references; avoid redundant tests and repeated full builds.

## Whole-package closure

PR-025 is implementation complete with local AC-025-01/02/03 passing and an independent source review finding no blocking core gap; its required cross-platform compile remains pending. No package is fully accepted. `IN_PROGRESS` means source, integration or required local verification remains. `IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE` means required source and local verification are complete, with specifically identified external/cross-platform/native verification or acceptance outstanding; it is not equivalent to accepted. Focused module test counts alone do not earn that status.

| Closure candidate | Source versus acceptance boundary | Required next closure action |
|---|---|---|
| PR-022 | Owning note reports source slices implemented and local tests/lint/guard/probe compiles passing. Owner confirmation of no remaining in-scope source gaps is requested before changing the package state. | AC-022-01/03: actual Linux/macOS neutral/shared-UI execution. AC-022-02: Unix native path and session persistence on foreign hosts. Unlocked event-loop probe interaction remains pending; unsupported native port capabilities are declared gaps, not claims of completed ports. |
| PR-023 | Owner confirms source gaps: Button/TextField/tab-strip painting still uses fixed dark tokens; full visible-item tree/list/tab semantic hierarchy is missing; product views do not uniformly consume EventRouter/FocusChain/adapters; full light/dark/high-contrast RecordingBackend goldens are missing. Sixteen tests/lint do not close these gaps. | Complete that source/integration checklist; separately verify physical IME/AltGr/dead keys, focus/pointer behavior, DPI and actual composed-screen equality when unlocked. Active split-view work is not PR-023 completion evidence. |
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
