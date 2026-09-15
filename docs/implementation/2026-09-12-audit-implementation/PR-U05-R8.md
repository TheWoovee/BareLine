# PR-U05-R8 — Recovery refresh after restore

## Baseline and evidence

Baseline `8eca399e663156d9ff2b74be86a57f46ee13f39a`. Native R7 evidence `target/qualification/final-native-audit/evidence/r7-recovery-restore-stale-entry.json` shows a successful dirty Untitled restore followed by the same one-byte Recovery Center row, whose preview later fails because its adopted checkpoint has retired. Earlier export/restore evidence remains `r7-recovery-export-restore-pass.json`.

## Plan

Bind a restored checkpoint suppression to the typed restore terminal's document identity. Remove it from the current rows immediately, exclude it from asynchronous discovery while that document remains open, and release suppression when its owning document closes so unresolved data remains discoverable. Preserve generation/root fences and derive completion notification counts from the filtered terminal result.

Add a bounded generated recovery fixture proving an adopted live checkpoint cannot return from a post-terminal scan while another real checkpoint remains visible. Root owns Cargo and native qualification.

## Export clarification

The R7 `Export Edits` artifact reports `complete: false`, unavailable original ranges, and a zero-byte segment while Restore reconstructs the one-byte Untitled document. This package does not change export: `crates/file-io/src/recovery/mod.rs::export_edits` explicitly exports owned transaction segments and a `gaps.json` report with `complete: false` and the unavailable original range. No linked contract inspected for R8 establishes that this edits-only artifact must duplicate the full restore output.
