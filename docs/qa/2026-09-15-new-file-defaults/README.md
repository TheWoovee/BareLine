# NEW-FILE-DEFAULTS — Focused verification

## Result

**The new-file EOL and encoding/BOM wiring is implemented with focused verification.** New documents receive policy metadata before publication, remain clean and do not gain an artificial Undo entry. Native Enter and smart indentation use the document EOL. Later settings changes affect subsequent documents; literal pasted text and existing mixed-EOL files retain their bytes. Explicit EOL conversion before the first newline is undoable.

| Check | Result | Evidence |
|---|---|---|
| Focused Rust filter `new_file_` | 6 passed: 4 app, 1 document, 1 editor-surface | [Final receipt](captures/focused-final-tests.json) |
| Adapter contract | 22 passed | [Receipt](captures/adapter-tests.json) |
| Incremental Windows debug build | Exit 0; 46.386 seconds | [Build receipt](captures/debug-build.json) |
| Native UTF-8 without BOM + CRLF | All 3 plain_text steps PASS; editor Exit 0 | [Receipt](captures/native-crlf-utf8-retry.json), [observations](runs/native-crlf-utf8-retry/scratch/native-observations.json) |
| Initial CRLF attempt | Stopped before typing: lost foreground; dependent steps NOT_RUN | [Receipt](captures/native-crlf-utf8.json) |
| Native UTF-8 BOM + LF | Stopped before typing: lost foreground; dependent steps NOT_RUN | [Receipt](captures/native-lf-utf8bom.json) |

The byte-level Rust cases verify exact UTF-8, UTF-8 BOM, UTF-16LE BOM and UTF-16BE BOM serialization, clean save, and unchanged mixed-EOL bytes after reopen/save under different defaults. They also verify LF/CRLF smart indentation, creation/Undo/Redo policy, an explicit EOL override before first Enter, supported ANSI code-page selection and rejection of unsupported code pages without publishing a tab. The earlier four-test receipt is retained as an intermediate source state, not added to the final six-test count.

## Source and candidate boundary

The native runs use SHA-256 `c3b618c1dadce5ecd3ed023357bf48aa35d4585f3b057ef8a5e9ea502cd02a4f`. The final tests and incremental build record stable dirty-source manifest `31dd01bc5fb601312a333da8419387ec32a3b4f38a925fb8657242b96cebf688`, at HEAD `e6c1486ff0bb997d96801331b32986d612728d4f`. Subsequent documentation edits do not relabel those receipts. [verification-summary.json](verification-summary.json) binds the implementation/adapter sources and candidate and lists every new native attempt.

The passing run is `8d4b2195-317a-467d-a845-2b183e8dee3d`, Windows 10.0.26200, software renderer, dark private profile, keyboard automation and observed 96-DPI window. It types astral/combining/CJK text and CRLF, saves exact bytes through native Save As, closes/reopens through native Open, checks clean/dirty transitions and Undo/Redo, and observes checked Exit 0. All native captures record stable source and bound artifact hashes. A final process check found no remaining Bareline process.

## Limits and remaining work

Both interrupted attempts stopped at the foreground guard before product input. The first was followed by one fresh CRLF run; a second interruption in the additional encoding cell ended native input for this slice. Their FAIL/NOT_RUN statuses are preserved. Native LF/BOM and UTF-16 cells, the host System encoding, other Windows floors/renderers/themes/DPI, physical IME and screen-reader qualification remain pending. The System setting uses Windows GetACP; code pages absent from the existing codec catalog produce an explicit error asking for a supported encoding.

There was **no full suite, aggregate native run, release build or acceptance import**. Independent/final review remains pending. QUAL-012's broader scope remains open; DEV-003 has twelve unimplemented product procedures. **47 top-level backlog items remain open.** Parity-038 returns to awaiting qualification; no capability or AC is fully qualified by this slice.

## Retention

[retained-files.json](retained-files.json) maps 88 exact-byte copies (978,438 bytes) to their original paths and SHA-256 hashes. It includes raw receipts/logs, all files from the three generated run directories, and inert source copies. Native references still point to original scratch paths; use the mapping to review retained copies. The [previous DEV-003 failures](../2026-09-15-dev003-product-adapter/README.md) remain unchanged as the pre-fix baseline.
