# File/document backend progress

Active briefs: PR-002, PR-004, PR-007; FC-01/02/03/04/06. This note records incremental backend evidence, not complete paging or codec acceptance.

Delivered actual file-backed source producer: metadata-only open, Resident/Paged selection (defaults 256 MiB/1 MiB pages/64 MiB paged cache), requested-page I/O, retained-page aggregate budget accounting, before/after handle and path identity checks, cancellation, and verified Resident sealing. Every fill call handles at most one page so the owning worker can prioritize viewport work. Cached pages stay immutable; unread changed-generation ranges become unavailable. Metadata checks do not defend against writers spoofing metadata.

`DocumentBuilder` incrementally builds shared immutable Resident roots without undo history or whole-file strings. `open_utf8_streaming` publishes one bounded prefix before finishing the remaining read, handles split UTF-8 scalars and BOM, validates complete source identity, and returns only a complete document on success. `IoRequest::OpenStreaming` / `IoTicket::try_prefix` expose one bounded progress slot. Prefix snapshots are explicitly incomplete, have distinct content states/revisions as loading advances, and cannot be normally saved or marked saved. UI consumption is a separate integration change; the legacy `open_utf8` one-page API remains compatible.

`PagedSnapshot` provides bounded source-backed UTF-8 window requests with `Pending`, `Unavailable`, invalid-UTF-8 failure, and unknown total line counts. A request owns its bounded prefix across cache eviction and resumes after its ticket is serviced; a split scalar across pages is validated only when the requested window completes. This is a read layer, not yet an editable source-leaf tree. Legacy bytes require a decoded source first.

Read validation is separate from replacement validation. Windows allows read-only and hard-linked sources through the same existing local-path/trust checks while replacement restrictions remain in force.

Headless evidence on Windows, 2026-09-06:

- `cargo test -p bareline-file-io source::tests --offline`: 3 passed. Generated 1 GiB sparse fixture reads only requested 4 KiB pages; bounded cache and outstanding page accounting; truncation/cancellation fail closed; Resident sealing; generated >2 MiB streaming UTF-8/BOM split-scalar round trip, prefix-before-completion, cancellation, incomplete-save rejection.
- `cargo test -p bareline-document paged::tests --offline`: 2 passed. Single-page-cache window resumption with split scalar, source change and malformed bytes, scratch-budget release.
- `cargo test -p bareline-platform-windows readonly_source_can_open_but_cannot_be_replaced --offline`: 1 passed.
- `cargo check -p bareline-file-io --offline`: passed after ticket wiring. Focused changed-file rustfmt used; no global formatting.

Remaining: editable source-backed tree with owned inverse materialization, sparse index, disk-backed transcode/spill and quota handling, live paged/read-only viewport integration, broad huge-file memory performance evidence. No locked desktop interaction was attempted. Graft callers navigation reported approximately 5,438 tokens saved (MemorySource and open_utf8_cancellable).


## Resumed file-core delivery (2026-09-06)

The preceding remaining list is an older checkpoint. Source now supplies the shared AVL source-leaf `PagedDocument`, owned materialized inverse edits, bounded asynchronous UTF-8 windows, sparse line metadata, atomic group backend, disk-backed transcode quota pause/resume, and staged streaming paged saves.

Actual consumer wired in `app::workspace`: `WorkspaceEditor::Paged` owns `PagedEditorSurface`; resident-cap refusal launches the paged transcode worker, a bounded loading prefix becomes a paged editor on completion, input/undo/redo and saves reach the paged actor, viewport paging advances with a 48 KiB overlap, and save-as updates authoritative path/fingerprint/clean content state. The renderer holds only an incomplete <=64 KiB adapter; it is never presented as a complete document snapshot. `PagedSnapshot::same_document` exposes stable identity independently of viewport adapter identity. CLI read-only policy blocks mutation/save. Successful native macro receipts are exposed through a bounded acknowledgement queue.

Correctness fixes: keep authoritative snapshot/save metadata when a post-mutation viewport read fails; prevent subsequent edits against the stale rendered window; retain quota-paused transcode on a saturated retry queue; create the cache parent on the worker; settle replacement progress after edits finish. Private raw/text/provenance stores accumulate SHA-256 hashes during transcode. Exports acquire three platform sealed-read guards before content verification and retain them over copying and final verification. Windows denies write/delete sharing; unsupported neutral platforms fail closed. This closes ordinary persistent and concurrent cache modification during export without trusting metadata alone.

Focused evidence: `cargo test -p bareline-app paged_workspace_edits_undoes_navigates_and_saves --lib --offline`: 1 passed, generated 200,000-byte fixture, bounded adapter, insert/undo/redo/page and save-as exact output/clean state. `cargo test -p bareline-file-io paged_quota_receipt_resumes --lib --offline`: 1 passed, retained quota retry, raw provenance, refused conversion and same-size sealed raw cache tamper with original target intact. `cargo test -p bareline-platform-windows sealed_read_denies_mutation --lib --offline`: 1 passed, write/truncate/rename/delete denied while guarded and access restored on drop. `cargo check -p bareline-app --offline`: passed after consumer/acknowledgement wiring. These are scoped consequential checks, not full acceptance or native visual proof.

Remaining acceptance: native paged journey and page navigation hook proof; generated multi-GiB working-set evidence; interactive editing before a legacy transcode finishes (current early prefix is read-only); dynamic free-space-based quota and Settings/UI resume controls; paged recovery journals/selection history and huge cross-window selection; complete fold/wrap/rendering and resident/paged feature parity. Core window paging is bounded byte-window navigation, not a completed arbitrary-line seek UI. Disk checks add two bounded full-store reads per export; performance evidence remains outstanding. No broad PR completion claim, commit, remote push or GUI launch was made by this lane.
