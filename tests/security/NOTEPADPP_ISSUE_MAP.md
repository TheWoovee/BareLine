# Notepad++ baseline issue classes → Bareline regressions

Verified against the linked official issue pages and official changelog on
2026-09-08. These are issue-class mappings, not claims that Bareline shares a
Notepad++ implementation or that an upstream report establishes a Bareline defect.
Every test below is source-located; this mapping records **NOT_RUN** for this closure
lane. Previous execution evidence must be attached separately by the coordinator.
No CVE identifier is inferred where the upstream changelog says unassigned.

## Baseline issue reports

| Verified upstream report | Bareline control / regression source | Coverage boundary |
|---|---|---|
| [#16922: slow search-result navigation](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/16922) | `crates/app/src/search_panel.rs`: `grouped_panel_draws_only_visible_rows_and_activates_exact_source`; PR019 result-jump measurement | Source regression covers target identity/virtual rows; matching large-file native responsiveness reproduction is UNCOVERED here. |
| [#17510: pinned tabs lost on update](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/17510) | `parser_corpus.rs`: `session_mutations_preserve_validation_and_pinned_roundtrip`; `crates/file-io/src/session/mod.rs`: `round_trip_and_migration_preserve_views_and_paths` | Pinned identity persistence is covered in source; installed update/restart journey remains UNCOVERED here. |
| [#18062: session saving in synchronized storage](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/18062) | `crates/file-io/src/session/mod.rs`: `failed_publication_preserves_original_and_corruption_recovers_previous`; corpus journal valid-prefix test | Atomic publication/corruption controls; actual sync-provider locking behavior is UNCOVERED here. |
| [#18355: column paste corruption](https://github.com/notepad-plus-plus/notepad-plus-plus/issues/18355) | `crates/editor-surface/src/power.rs`: `rectangle_unicode_tabs_short_lines_and_all_bases` | Unicode rectangle transaction/undo coverage; this is not an exact native clipboard reproduction of the report. |

## Security classes in the baseline's release source

The official [8.9.7/8.9.8 changelog](https://github.com/notepad-plus-plus/notepad-plus-plus/wiki/Changes)
is the verified source for the class labels below. Its listed fixes include UDL and
UTF-16 loading, array writes, signature verification, updater races, backup traversal,
UNC credentials, macro authorization, session handlers/path lengths, launch targets,
installer interpolation, and window-message integrity boundaries. The mapping does
not equate these upstream fixes with complete Bareline acceptance.

| Class | Bareline control / located regression | Remaining evidence |
|---|---|---|
| UDL parser crash | `parser_corpus.rs`: `udl_mutations_reject_entities_and_preserve_registry_on_error` | Fixed mutation source; sanitizer/long fuzz campaign not performed. |
| UTF-16 loading / native array bounds | PR007 streaming codec provenance; PR001/008 FFI ownership | Exact upstream-trigger regression not supplied in this corpus: UNCOVERED. |
| Authenticode bypass | `crates/platform-windows/src/update.rs`: `unsigned_held_file_rejected` | Authorized-signer positive/negative native vectors require separate evidence; unsigned rejection alone is insufficient. |
| Update TOCTOU | `crates/extensions-protocol/src/package.rs`: `signed_offline_install_tampering_freshness_and_remove`; package retains verified bytes | Extension package swap is a related regression, not an updater apply race; updater native race remains separate. |
| Archive traversal | `package_corpus.rs`: `authenticated_package_mutations_reach_manifest_and_archive_guards` | Signed ZIP path/device mutation coverage; native reparse races remain separate. |
| Backup path traversal | Generated journal/segment names and `journal_mutations_preserve_valid_prefix_without_promoting_bad_tail` | Corrupt-tail integrity is covered; malicious deletion/reparse end-to-end attack is UNCOVERED here. |
| UNC credential exposure / session normalization | `crates/platform-windows/src/path_trust.rs`: `remote_and_metadata_paths_rejected_without_access`, `approved_local_session_is_read_only_and_remote_stays_blocked` | Native trust-gate source tests; packet-level zero-contact observation not supplied. |
| Macro authorization bypass | `crates/macros/src/process.rs`: `grants_and_launch_errors_are_visible_without_starting_a_process` | Explicit direct/shell grants; imported macro UI authorization replay requires separate acceptance. |
| Session null/overlong path input | Corpus session malformed identities/lengths; `crates/platform/src/paths.rs`: `validation_rejects_nul_and_oversized_state` | No raw Notepad++ session-message ABI exists; Bareline native malformed IPC tests remain separate. |
| Workspace target hijack / installer interpolation | Path trust and literal argv; `crates/macros/src/process.rs`: `direct_placeholders_keep_quotes_metacharacters_and_native_paths_literal` | Related direct-launch invariant only; installer/Explorer target race is UNCOVERED here. |
| Lower-integrity messages / malformed IPC | `parser_corpus.rs`: `rpc_mutations_revalidate_and_oversized_prefix_never_reads_payload`; authenticated pipe broker | Framing is covered; Windows peer/integrity spoofing requires native tests, not parser mutations. |

## Extension compatibility class

The baseline also cites the official community [plugin compatibility FAQ](https://community.notepad-plus-plus.org/topic/23146/faq-notepad-crashes-freezes-unresponsive-after-update).
Its body was rechecked on the same date and describes plugin communication and
compatibility changes. Bareline's analogous controls are version negotiation,
out-of-process execution and `crates/extensions-protocol/src/broker.rs` tests
`denied_scopes_revocation_and_timeout` and `stale_and_bad_ranges_and_update_rejected`.
These do not establish native host-crash isolation; that journey remains separate.

Open coverage is deliberately visible. This artifact completes traceable mapping,
not the PR020 acceptance checklist or proof that every attack class has been exercised.
