# Ten sequential readiness defects - 2026-09-15

**Scope:** user requested ten selected defects, addressed one at a time without parallel agents. Preserve prior changes and evidence. No full suites or large release builds; focused regression checks per fix and one necessary incremental app compile. No commit, push or publishing.

These are concrete defects within QUAL-024 and DEV-005/QUAL-021, not ten completed top-level backlog packages. The separate ten unimplemented native procedures are unchanged by this batch.

| Order | Defect / observable failure | State |
|---|---|---|
| 1 | Regex mode's accessible label is overwritten by the generic Literal/Extended command title. | fixed; focused test passed |
| 2 | Duplicate JSON keys can overwrite an earlier failure/identity field during evidence parsing. | fixed; focused test passed |
| 3 | Boolean/fractional schema and receipt accounting values can pass integer comparisons. | fixed; focused test passed |
| 4 | Imported journey results need not match the requested journey and canonical captured procedure. | fixed; focused test passed |
| 5 | Generic journey execution can report PASS after the pinned executable changes during the run. | fixed; focused test passed |
| 6 | Arbitrary generated files can be attached as fixtures without being bound to the executed result. | fixed; focused test passed |
| 7 | Imported environment labels can be changed, and input/theme/DPI identity is dropped. | fixed; focused tests passed |
| 8 | Whitespace/case variants of one reviewer name can bypass the independent-review check. | fixed; focused test passed |
| 9 | A matching result binding is accepted even when raw output contains conflicting bindings. | fixed; focused test passed |
| 10 | Receipt logs are unbounded; JSON can grow past its pre-read size check. | fixed; focused test passed |

## Authority and workflow

PR-024/FC-11 and UI-FN govern accessible control names. PR-021 and the maintained [evidence contract](../../parity/README.md) require exact source/binary/fixture/environment identity, checked execution and independent review. DEV-005 tracks broader typed evidence and environment aggregation; this batch strengthens existing validation without claiming those larger features are complete.

For each item: inspect the owning graph/source, reproduce with a targeted regression, apply the smallest coherent fix, run the focused check, retain red/green receipts and update this ledger before starting the next item. Synthetic parser/producer tests are not native product acceptance. Final reporting separates ten selected fixes from remaining top-level work.

### 1 — Result

Preserve the Find mode control name during command projection. One focused Rust test covers both mode controls across Literal/Extended/Regex, values, selected state and available actions. `01-repro.json` captures the wrong-name failure; `01-green.json` passes. `01-red.json` is a retained initial test-fixture setup error, not the defect reproduction. Physical screen-reader qualification remains pending.

### 2 - Result

Both evidence readers reject duplicate JSON fields at every object depth; the shared parser is included in native adapter source hashes. 02-red/02-green preserve reproduction and passing focused check.

### 3 - Result

Evidence schemas and receipt/xtask counters require actual integers. Boolean and floating-point substitutes are rejected; 03-red/03-green cover the reproduced false-success values.

### 4 - Result

Imported results must match the requested journey and the complete captured canonical journey, including actions/expectations and request schema. Missing/altered legacy procedures cannot qualify current mappings. 04-red/04-green preserve the focused reproduction.

### 5 - Result

Runner verifies and records the executable hash after owned Job cleanup. A passing response followed by a binary change now produces FAIL; the unchanged control passes. 05-red/05-green use an isolated synthetic producer, without launching the generated binary.

### 6 - Result

Both adaptation and resolution require fixtures to be declared in the captured step artifacts. All declared artifact hashes and scratch containment are revalidated; unrelated files and changed bytes are rejected. The unchanged bound-fixture control passes. 06-red/06-green/06-control retain focused evidence.

### 7 - Result

All five captured environment fields are required, preserved and revalidated. Missing fields and CLI overrides cannot supply a different execution environment. 07-red/07-green/07-complete retain the reproduction and two passing regressions. Status-only xtask observations still cannot qualify an acceptance criterion.

### 8 - Result

Importer and resolver normalize reviewer names with Unicode NFKC, whitespace folding and case folding before requiring distinct nonempty names. 08-red/08-green cover four spelling variants in both paths. Reviewer names remain human assertions; this does not authenticate people.

### 9 - Result

Require exactly one complete result binding across structured and legacy capture lines. Reject conflicts, duplicates, mixed bindings, incomplete legacy pairs and duplicate JSON fields. Valid structured/legacy controls pass; 09-red/09-green retain the focused reproduction.

### 10 - Result

Read at most 16 MiB plus one sentinel byte for each evidence JSON file or receipt output stream. Reject excess bytes and hash the same bounded log bytes used for capture binding. Exact-limit controls pass; oversized JSON/stdout/stderr are rejected. 10-red/10-green retain the focused boundary regression with small injected limits.

## Final checkpoint

All ten selected defects are fixed in sequence. The affected cohort passes: 11 new evidence regressions, 14 runner tests, 13 adapter-exit tests and 22 native-adapter tests (60 distinct Python tests), plus one Rust accessibility regression. The 13 adapter-exit checks were rerun after applying the actual 256 KiB response-read bound. One incremental debug app build passed in 8.281 seconds.

[Retained receipts and source snapshots](../../qa/2026-09-15-ten-defects/README.md) preserve each reproduction, passing check and the initial Rust test-fixture error separately. Full suites, release builds, native recapture and physical screen-reader checks were deferred. No independent reviewer, AC import or capability promotion is claimed.

**Remaining: 0 of this selected batch; 47 of 49 top-level readiness items remain open.** DEV-005 still needs typed producers and required-environment aggregation. DEV-003 still has ten unimplemented native procedures, starting with column_multi_cursor. These counts describe different scopes and are not additive.
