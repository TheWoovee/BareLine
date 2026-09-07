# Bareline Foundation Contracts

**Version:** 1.3 · **Date:** 2026-09-05 · **State:** specified; prototype evidence pending.

This document supplies the concrete contracts adopted by ADR-37–46. It is subordinate to the decision log. These are implementation requirements, not claims about existing code. The original v1 scope and non-gating performance policy remain in effect.

## FC-01 Text, raw bytes and codecs

The logical text presented to text consumers is valid UTF-8. Raw file bytes are a separate immutable-generation byte domain. Public types distinguish `RawOffset`, `TextOffset`, `Revision`, `SourceGeneration` and `ContentStateId`; implicit conversions are forbidden.

Decode invalid input into U+FFFD display spans with a **side table** mapping each replacement span to original bytes and source offsets. An ordinary U+FFFD or private-use character has no side-table entry and is never interpreted as an escaped byte. Do not encode byte provenance inside Unicode character values. Text edits cannot split an opaque invalid span. Copy as text produces replacement characters; an explicit raw-byte export preserves bytes.

`DecodedSpan { text_range, raw_range, provenance: Original | InvalidBytes | Inserted, codec }` records source provenance. Untouched original spans are copied byte-for-byte when the save encoding is unchanged. Only inserted/changed spans are encoded. Cross-span codec state must be represented for any stateful codec; v1 does not support stateful ISO-2022 encodings. Conversion to a different encoding refuses unresolved invalid spans until the user selects an explicit replacement policy. A no-op save cannot normalize aliases or legacy mappings.

Initial supported set: UTF-8 with/without BOM; UTF-16LE/BE; UTF-32LE/BE; Windows-1250–1258; ISO-8859-1; Shift-JIS, GBK, Big5, EUC-JP and EUC-KR. `encoding_rs` supplies only its supported codecs; dedicated, tested Unicode encoders/UTF-32 codecs and a true ISO-8859-1 codec fill its gaps. Do not silently map ISO-8859-1 to Windows-1252. Record aliases, BOM policies, detection confidence and codec implementation/version in a checked-in catalog. Add more code pages only with round-trip fixtures and a catalog row.

Detection reads a bounded 64 KiB prefix, considers BOM first and validates UTF-8 incrementally. It never scans an entire huge file before publishing the viewport. Later invalid bytes create visible invalid spans; they never silently change encoding. Interpret-as requires confirmation before discarding dirty edits, uses a verified original generation and can fail with SourceUnavailable. Convert-to changes save policy and is undoable metadata state.

## FC-02 Sources, unknown indexes and memory

`ByteSource` returns `Ready(bytes, generation) | Pending(ticket) | Unavailable(reason)`, never a fabricated zero-filled range. UI reads are nonblocking and trigger background prefetch. Resident and Paged share this contract. Resident becomes sealed only after its copy is complete and source checks succeed. Until then, source mutation can invalidate unread ranges exactly as for Paged.

A snapshot owns a persistent text root, source-generation references and available-range metadata. Published Ready bytes never change. An external backing file is not an immutable snapshot: uncached ranges can become Unavailable. Jobs must propagate this as incomplete/error; document revision alone is insufficient to validate results. Strict jobs use a sealed private source or explicitly accept incomplete reads. Check source identity/size/write metadata before and after a read, discard mismatches, and do not claim this prevents an adversarial writer from spoofing metadata. Full immutable-source guarantees require a private sealed copy.

Large decoded text is stored in a private **disk-backed paged transcode store**, not the in-memory insertion store. Retain original raw spans and mapping checkpoints. Partial transcoding has an available text length and unknown final length. Cancellation removes unreferenced temporary segments; disk-full pauses the operation and exposes Retry / Change cache location / Cancel. No destructive cleanup of live recovery references is allowed.

Line aggregates are `Known(count) | Unknown`; never label an estimate exact. Treat CR, LF and CRLF as line terminators, with a node summary containing first/last boundary bytes so a CRLF split across pieces counts once. Empty document has one logical line; a trailing terminator introduces an empty final logical line. Sparse lookups outside indexed regions return Pending with cancellable progress. Byte-based navigation stays usable without exact total lines. Editing unread content first materializes the affected range; no speculative delete of unknown bytes.

Default budgets (engineering settings, tune with published evidence): 1 MiB source pages; 64 MiB maximum cache per active document; 256 MiB aggregate source/decoded cache across the process; 128 MiB aggregate undo RAM with disk spill; 64 MiB aggregate search-result RAM; 16 MiB clipboard history across at most 20 entries, maximum 4 MiB per entry. Resident loading participates in the aggregate budget: when many small documents exceed it, spill sealed chunks to owned disk storage and cache pages on demand. Per-file Resident eligibility is not permission to allocate 256 MiB for every tab. Full-file reads must never accumulate above these budgets. Background queues are bounded; low-priority tasks yield to viewport and edit work. Settings show effective limits. Cache/undo/result/spill counters are local diagnostics only; no telemetry upload.

Temporary transcode quota defaults to min(20 GiB, 20% of free volume space), checked before and during growth. Crossing a quota pauses with a choice; it never silently discards recovery. Recovery storage is separately accounted and requires explicit retention management. Large edits/replace-all can spill to a transaction store, then commit atomically as one root change.

## FC-03 Recovery and durable edit ownership

Before a transaction commits, inserted bytes and inverse bytes for replaced/deleted ranges must be owned independently of evictable base pages. Undo references these immutable edit/recovery segments. View-cache retention is never an undo strategy. A committed transaction is distinct from its durable acknowledgment.

Journal order: write required byte segments → flush segments → append transaction with segment hashes/lengths and CRC32C → flush journal → publish durable revision. A checkpoint becomes current only after its referenced stores are durable and its manifest is atomically replaced. Keep the previous checkpoint until the new one is committed. Durable manifests include source generation, codec catalog version and format version. Recovery interval is at most 5 seconds or 256 KiB of edits, whichever occurs first, **while storage is writable**; storage failure creates a persistent Recovery unavailable banner with Retry and Save As when the source is complete, or Export available data when it is incomplete; never a false success acknowledgment.

Complete recovery of unchanged base content requires either the original verified generation or a sealed private baseline. First dirty edit schedules a cancellable, disk-backed baseline copy; source changes during copy abort sealing and preserve journaled edit payloads. User edits remain interactive while this copy runs; UI says “Edits protected; full snapshot preparing” until sealed. This honest state is necessary because bounded-memory paging cannot recover original bytes that were never copied and subsequently disappeared. An incomplete recovery item offers inspect/export of recoverable edits and owned ranges, marks every unavailable range, and never claims to reconstruct the full file. Complete recoverable untitled documents need no external baseline.

Recovery Center labels preview panes On disk / Recovered and distinguishes Complete / Edits only / Corrupt tail recovered / Source unavailable. Restore never writes the original file. Discard requires confirmation and a durable tombstone before garbage collection. Keep for later and closing the panel preserve all data. Garbage collection traces every live checkpoint, undo and recovery reference before deleting a segment. Tests kill the process at each durability boundary and modify/delete the original before recovery.

## FC-04 External changes, tail, savepoints and saving

Paged ordinary editing freezes unresolved reads on overwrite/truncation/replacement. Cached immutable bytes remain available. Whole-document Save/Save As is disabled when any required range is unavailable; Export recoverable ranges is a separate action producing data plus an unavailable-range manifest. It must not silently concatenate disconnected ranges.

Tail is a specialized append generation. Verified growth preserves the previous prefix and creates a new logical revision, including a copied partial final page. If append-only continuity cannot be established, treat it as SourceChanged and offer Reopen and follow; never merge uncertain bytes. Pause stops auto-scroll, not bounded ingestion. Unlock to edit stops following and captures a fixed source generation after confirming consequences. Resident loading, append, truncation, rotation, watcher overflow and replacement have separate tests.

Job `Revision` is monotonic; `ContentStateId`/savepoints model edit history. Dirty is whether current content/encoding/EOL save policy equals the saved state, not monotonic revision inequality. Undo to a saved state becomes clean. A save captures `(snapshot, source_generation, content_state)`; only that state becomes saved after replacement. Newer edits remain dirty. Concurrent saves to one target are serialized.

For local NTFS, stream to a private same-directory temp, flush, revalidate expected target identity/version and replace using the platform adapter. Capture and compare full source fingerprints when approving a destructive preview. Keep an original backup generation during replacement where policy requires it. Ordinary external writers can still race a last metadata check: preserve conflict artifacts and report this limit; do not promise atomic compare-and-swap against arbitrary other processes. No fallback truncates the original.

Metadata policy: preserve ACL/security descriptor and supported alternate data streams; update modification time for changed content; preserve creation time where supported; preserve user attributes unless explicitly changed. A read-only attribute causes a refused save until explicitly cleared. Hard-link count >1 prompts: replace only this directory entry (break the link) or cancel; never silently switch to in-place write. Resolve reparse targets only after trust approval and display actual target. NTFS is the reference reliability filesystem. FAT/exFAT, SMB and sync-provider folders have a capability report and conservative Save a Copy fallback where atomicity/ACL guarantees are unavailable. Portable mode promises no app registry/config writes outside its directory, not erasure of OS-managed caches or forensic secure deletion.

## FC-05 Search and replacement semantics

Read windows are at most 16 MiB and end on a valid UTF-8 boundary; prefer line ends where possible but permit long-line splits. Literal scanners retain pattern-length context, with an explicit pattern-size cap. Unicode folding preserves a folded-to-text offset map. Results include raw/text domain, source generation, document revision, query ID and completeness. Never treat estimated line numbers as exact.

Do not infer regex multiline ability by searching the pattern string. PCRE2 patterns use one bounded-context execution path with per-job match/depth/heap limits and callout/interrupt checks. Partial matching appends retained subject context and reruns; use correct BOL/EOL and subject-anchor handling, retain lookbehind context, and advance empty matches by one Unicode scalar according to the documented engine mode. PCRE2 \A/\z, \G, lookbehind, backreferences and end-sensitive alternatives need compatibility fixtures. If the pattern cannot be proved correct with the context retained, use a complete bounded subject or return UnsupportedForStreaming; never silently miss matches. The compatibility catalog lists supported versus restricted constructs. Max retained subject/match context is 64 MiB by default. A limit returns Incomplete with a reason; users may raise a limit explicitly within the global resource policy.

No destructive replace applies from Incomplete, Cancelled or Unsupported results. A complete replacement plan stages match identities, expanded captures and edits before one document transaction. Large staged data spills to disk. Known-byte result jumps request viewport pages only; stale results revalidate or show stale state. Search completion, stop acknowledgment and total count have independent state fields.

Workspace replacement has two explicit phases. Preview records identity, full file fingerprint, size, encoding/BOM/EOL, query/options and selected match IDs. At apply, revalidate against that exact reviewed version; mismatch or a file becoming a dirty open document is skipped with Review changes again. Do not rematch a newer file and apply unreviewed changes. Open documents use their captured revision and become dirty without autosave. Closed files use atomic save. Persist per-file Planned/Staged/Committed/Skipped/Failed outcomes; flush a commit receipt before moving to the next file. After crash reconcile any uncertain outcome by fingerprint, report it and never blindly repeat. Backups use generated collision-resistant names and a manifest; existing .bak files are never overwritten. Bulk operations are not globally atomic: completed files remain changed after cancellation and closed-file undo requires retained backups.

## FC-06 Scheduling, transactions and diff

One logical DocumentService mailbox per document runs on a bounded scheduler (default 2 workers, configurable up to logical CPUs). Actors serialize their own mutations but do not own OS threads. Separate bounded I/O/search pools, one watcher thread and lazy extension IPC are permitted to sleep at idle. Idle means no polling loop and negligible CPU; timers suspend when unnecessary. Queue saturation coalesces replaceable work, cancels stale jobs and backpressures non-droppable edits.

Cross-document drag moves use a coordinator: validate both revisions, stage both inverse payloads, commit linked roots in document-ID order, and attach one UndoGroupId. A failure before commit changes neither document. Undo is coordinated only while both histories still contain the linked savepoint; otherwise refuse with an explanation, never perform half a move. Same-document moves remain a single transaction.

Diff hunks contain both byte ranges and logical-line hints, both source generations/revisions, normalized comparison options and stable anchor-based IDs. Use one whitespace enum Significant/TrimEdges/IgnoreAll, plus ignore blank lines, case, EOL, encoding/BOM and tab normalization. Compare normalized bytes after normalized-hash collisions; raw differences are allowed when explicitly ignored. Accept-all equals target bytes only under exact/no-ignore comparison. With ignore options, assert equality under normalization and preserve ignored destination content according to explicit merge policy.

Anchor maps, intraline spans and emitted hunks share the resource limit. Sample/coarsen before exceeding it; output can use bounded batches rather than Vec for every line. Return Completed(Exact/Coarse), Cancelled, SourceUnavailable or Failed; cancellation is not a requirement to finish a coarse diff. Apply hunks requires matching revisions/generations and complete source ranges. Coarse hunk copies preview their full affected range. PR-010 owns an AlignmentMap of view-only spacers and bidirectional scroll mapping; spacers have no document line number.

## FC-07 Extensions and trust

The core installer contains no runtime pack. Once installed, disabling all extensions stops the host and its IPC; it does not delete the runtime. Offer Remove runtime explicitly for disk reclamation and retain offline usability. Distinguish installed, enabled, permitted and runnable states. First-party packages are available in the signed catalog and an offline bundle; they are not implicitly granted capabilities.

PR-001 defines `VerifiedPackageSource`, path-trust and codec interfaces. PR-016 consumes a fail-closed download stub until PR-018 supplies network/download integration; offline verified install is usable first. Only PR-018 owns production package retrieval. Never download unverified code through a temporary bespoke path.

WASI capabilities are deny-by-default and scoped per extension to document IDs, workspace roots and approved operations. The editor authenticates the host connection with per-user ACLs, a launch nonce and expected process identity. Per-extension stores enforce default 128 MiB linear-memory limit, 5-second interactive operation deadline, cooperative cancellation and instruction/epoch interruption for CPU work; long operations need explicit cancellable task budgets. Kill/restart host on a violated bound; other extensions may restart but the editor remains intact. Revocation cancels pending requests, closes handles and rejects queued replies.

IPC has an 8 MiB frame limit and 1 MiB data chunks. Formatter results upload a staged transaction in chunks, then CommitEdits(base_revision); no frame carries a whole GB document. Document APIs distinguish read_text_range and read_original_bytes. Hex labels Original bytes (disk generation) versus Current encoded preview, with separate offsets and dirty-state messaging. XML and Hex panels request ui.panel. UI contributions are declarative controls/data, not HTML/script execution or native window handles.

XML disables external entities, remote DTD/schema fetch and XInclude; caps depth and expanded text. XPath v1 supports child/descendant paths, attribute and text selection, namespaces, and bounded equality/position predicates; unsupported axes/functions return an explicit message. JSON formatting preserves number lexemes and string values, caps depth and uses incremental tree indexing. Import formats and language definitions are data-only and resource-limited.

## FC-08 Update trust and signing

Signed metadata binds channel, artifact type, OS/architecture, version, length, hash, minimum compatible protocol and expiry. Persist highest accepted metadata version per channel. Reject rollback, expired metadata, wrong platform/channel, and unexpected signer identity. Offline use continues; inability to check freshness disables installing a new update rather than disabling the editor. Authenticode validation must match an authorized publisher/key policy, not merely any trusted certificate.

Use a pinned root metadata policy with separated release/catalog keys and an offline recovery authority; rotation requires signatures from old and new root policies. Bootstrap current keys in installers and publish fingerprint verification instructions. A compromised sole trust root cannot revoke itself safely: recovery then requires an independently authenticated new installer/manual trust reset. Specify that limit. Use an established update-framework design and vetted primitives; PR-020 validates rollback/freeze/key-compromise test vectors before shipping.

Release order: build deterministic unsigned inner executables → compare unsigned normalized payload hashes in two clean environments → owner-approved Authenticode signing → construct final installer/ZIP → sign installer → generate SBOM/notices and final artifact hashes → owner signs checksum/catalog/update metadata with offline minisign → CI verifies signatures against pinned public policy → publish draft → owner releases → publish update pointer last. Include final packaged contents in the SBOM. Timestamped signatures and containers are not expected to reproduce byte-for-byte; publish the unsigned payload comparison and signed final artifact hashes separately. Signing credentials are never included in the repository.

## FC-09 Platform and configuration contracts

Initial test baseline: Windows 10 22H2 build 19045 and Windows 11 23H2 build 22631 or later, x64. These are product compatibility floors, not claims about Microsoft support entitlement. Test on clean standard-user accounts; capture exact OS build in results. Later floor changes require release notes and an ADR.

Paths stay OsString/PathBuf. Persist a tagged lossless platform path representation (Windows UTF-16 code units or Unix raw bytes encoded in base64) plus a separate display string. Do not reconstruct paths from lossy labels. Named-pipe requests validate nonce/process/user/session and cap request sizes; duplicate launches cannot share a recovery writer.

Workspace settings are opt-in and allow only editor display, indentation, language associations and search excludes. They cannot grant extensions, change updater endpoints, execute processes, redirect recovery storage or enable remote access. Global/user policy owns security-sensitive settings. Modelines may select bounded language/indent settings only.

The initial paired lexer list is C, C++, C#, Java, JavaScript, TypeScript, Python, Rust, Go, HTML, CSS, JSON, XML, SQL and TOML. All other catalog entries declare Lexilla support/fallback explicitly. Provisional lexing never provides authoritative fold/comment semantics from a guessed blank-line state. Real checkpoints or conservative plain text are used until state is verified.

## FC-10 Measurements and evidence

Record first frame, first editable viewport, full resident load, first styled viewport, first durable edit and sealed recovery baseline separately. Performance budgets remain informational; correctness checks do not. Reference comparisons pin Notepad++ 8.9.8 artifact hash/settings for this baseline; a newer stable version is a separately labeled run, never silently mixed into a series.

Minimum sampling: 30 warm launches, 10 cold launches with documented cache-reset/reboot procedure, 30 file-open samples per size when practical and at least five repetitions of long throughput workloads. Publish every raw sample, median, P95, exclusions with reasons, timeout threshold and environment. Alternate application order to reduce cache bias. Record empty, 100-tab and 500-tab sessions; core-only and first-party-extensions-enabled footprints; peak RAM, process tree, disk spill and runtime download size. Hosted CI trends are separated from controlled reference-machine comparisons. Do not infer that Notepad++ fails a huge-file scenario; measure completion or timeout.

## Sources and implementation verification

- [encoding_rs limitations](https://docs.rs/encoding_rs/latest/encoding_rs/) — UTF-32 and UTF-16 encode paths require separate implementation.
- [PCRE2 partial/multi-segment matching](https://www.pcre.org/current/doc/html/pcre2partial.html) — context retention and rerunning are explicit work.
- [The Update Framework specification](https://theupdateframework.github.io/specification/) — use its trust/freshness principles and test vectors.
- [Mozilla MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/) — contributions and relicensing rights need explicit treatment.

Pins and prototypes must confirm library behavior before code acceptance. Documentation completion never marks an implementation or test DONE.
