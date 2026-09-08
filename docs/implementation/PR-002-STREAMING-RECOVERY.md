# PR-002 streaming storage and recovery integration

Source implementation checkpoint; automated validation is queued for the shared gate. No local Cargo, native build, or manual acceptance was run for this change.

- `owned_store::StreamingStoreBuilder` accepts bounded UTF-8 writes and source ranges, checks cancellation and configured disk/free-space quota, fsyncs, verifies the held sealed bytes against the write hash, and returns a source retaining the sealed loader. Invalid/incomplete UTF-8 and failed writes cannot publish.
- Recovery journal v3 retains the existing bounded CRC metadata envelope and streams inverse/forward payloads. The legacy 16 MiB vector replay cap remains unchanged; the new replay visitor returns sealed source ranges. Payload and recipe disk use share the supplied quota.
- Root recipe v2 stores owned text in a hashed, sealed side file and uses compact ranges. Original source references retain opaque-byte provenance. Recipe JSON remains bounded at 65,536 pieces / 128 MiB; those limits were not raised.
- The paged actor obtains the core's fully prepared exclusive commit lease, prepares the per-revision recipe, appends the flushed journal, then publishes the core mutation without allocation. Latest-pointer/checkpoint maintenance errors retain the durable receipt; restart can select the matching per-revision recipe. Undo/Redo stream history snapshots and no longer materialize a 16 MiB-limited inverse vector.
- Source and ordinary prepared edits expose nonconsuming operation receipts tied to captured document/revision. Targeted Resident promotion preserves the exact actor identity/history, including untitled Save As behavior. Native power/compare consumers use the shared workspace budget and these receipts.

Queued regressions: 18 MiB journal/source replay with bounded cache; interruption before/after journal flush; split UTF-8 scalars, incomplete UTF-8, quota, cancellation; actual paged 18 MiB edit → Undo → Redo → restart → save and compact recipe; malformed UTF-16 original edit → Undo → restart → exact raw-byte save, including a stale latest pointer.

The test filesystem does not prove Windows share-mode exclusion; that production capability remains supplied by the existing Windows `open_sealed_read` implementation and its native platform tests. Power/compare command consumers and aggregate core accounting are owned by their respective integration lanes.

Quota follow-up (source only): metadata-only and zero-payload streamed records now use checked disk admission. Journal CRC framing and atomic checkpoint staging are reserved before payload creation. Recipe JSON and owned bytes share one checked writer allowance, including historical-receipt and latest-pointer staging reservations. Failed preparation removes only files created by that attempt; uncertain journal failures preserve their evidence. Added zero-payload over-quota / near-framing and empty-owned recipe/receipt quota fixtures for the next gate.
