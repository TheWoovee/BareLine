# Windows v1 scope and known limits

Decision authority: the owner's 2026-09-16 delegation and ADR-47 in `docs/blueprint/07_DECISION_LOG.md`. This disposes INV-001; it does not waive the P0/P1 release gate or claim physical/security qualification.

| Limit | Windows v1 decision | Evidence / remaining qualification |
|---|---|---|
| Stateful encodings (for example ISO-2022) | Unsupported; never silently claim lossless round trips. Use a supported encoding after explicit conversion outside Bareline when needed. | FC-02; `crates/file-io/src/codecs/CATALOG.md`; codec tests in the consolidated workspace run. |
| Huge-file regex context | Partial/unsupported searches remain visibly incomplete; replacement requires complete, revalidated results. No complete-match claim from a limited window. | ADR-41/FC-05; `crates/search/src`; QUAL-005/019 cover the final candidate. |
| Recovery without a sealed original baseline | Export Edits may retain edits plus a gap report. It is not reconstruction of missing original content. | ADR-39; `crates/file-io/src/recovery/mod.rs::changed_baseline_and_cancellation_leave_honest_edits_only_export`; QUAL-004/020. |
| Pending global line index | The `~` line cue and progress are intentional until a verified index is available. | U13 and FC-02; native huge-file/index qualification remains. |
| Offscreen/very large accessibility ranges | Bounded Pending/Unsupported responses are intentional. No full screen-reader compatibility claim follows from unit tests. | PR-024; `crates/platform-windows/src/accessibility/text_provider.rs::five_gib_range_window_is_bounded_at_an_offscreen_offset`; physical Narrator/NVDA and long-selection checks remain QUAL-024. |
| Extension OS hardening | Current restricted token, Job, authenticated broker and capability-limited WASI remain the implemented boundary. Additional low-integrity/AppContainer work is future hardening. Do not describe the current host as AppContainer-isolated or as proven against native runtime escape. | ADR-43/FC-07; `crates/platform-windows/src/process.rs::restricted_token`, host sandbox/isolation tests. Actual signed deployment and the remaining threat matrix are still mandatory QUAL-016/020. |
| Module splitting / legacy lint debt | Maintenance, unless an observed behavior or contract violation demonstrates release impact. Fix new warnings; retain the exact existing warning ratchet. | Scoped T07/T08/T16 worker fixes; 2026-09-16 ratchet has no new/stale occurrences and removes one old occurrence. |
| Foreign hosts | Real Linux/macOS host qualification moves to the next update. Windows cross-platform code compilation is not foreign-host testing. | Explicit owner scope; REL-008 deferred. |

Known qualification gaps are described in the live backlog, not disguised as accepted bugs. Any new data loss, corruption, arbitrary-code/update, credential exposure, startup crash or unrecoverable-session defect must be fixed and verified before release. Sole offline-root compromise still needs an independently authenticated replacement installer or explicit manual trust reset.
