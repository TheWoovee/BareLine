# PR-006 paged transfer closure

Source implementation in closure-final. No Cargo, build, network exercise or manual QA was run by this owner. The coordinator must validate the queued tests before acceptance.

The native drag consumer captures real global selections and mapped drop offsets. Resident endpoints use targeted history-preserving promotion; the editor transfer worker stages UTF-8 ranges through the sealed quota-limited store and validates grapheme boundaries and exact captured identities. Same-document moves use one source transaction. Cross-document moves lease every actor in stable identity order and publish one linked undo group. Copy keeps the source unchanged.

Original byte provenance is retained across foreign source generations through sealed codec capabilities, including malformed UTF-16 spans. Recovery recipes reference retained foreign triplets rather than substituting replacement-character text. Source readers retain sealed handles and use bounded reads.

Every group future recipe is flushed before per-member intents and one common atomic commit marker. Restore validates all member recipe and owned hashes. An incomplete next preparation falls back to its previous committed group; later ordinary durable journal revisions supersede that group floor even when the latest pointer failed. Post-marker journal continuation failure poisons the old writer until recovery is rebased/retried. Marker publication precedes infallible actor publication; independent delete/insert acknowledgements are not used.

Transfer completion waits for each available participant view to install the committed revision and captured selection. Closed, failed or advanced views receive an explicit committed-but-installation-failed terminal result. Linked Undo/Redo retain per-document terminal results; completed closed participants are pruned without consuming live participant results. The bounded group registry retains actor dependencies required by linked history.

Queued meaningful regressions: malformed UTF-16 move across documents followed by restart and exact raw save; linked Undo/Redo; ordinary edit after a group with failed latest pointer; post-marker free-space/admission failure with poisoned old writer; and closing one participant before a subsequent linked Undo/Redo. Core owner also added all-member lease, wrong-member rejection, allocation-free publication, and multi-range provenance fixtures.
