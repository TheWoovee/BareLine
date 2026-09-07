# PR-005 search engine continuation

State: bounded regex fallback implemented; full PR-005 remains IN_PROGRESS.
Authority read: PR-005 brief, FC-05/06, ADR-09 and ADR-18. Dependency: pinned `pcre2-sys =0.2.10`, the low-level bridge for approved bundled PCRE2. It builds bundled PCRE2 on this Windows toolchain. Non-Windows builds must set `PCRE2_SYS_STATIC=1` to prohibit the dependency's system-library discovery; cross-platform compile has not been run here.

`scan`, `SearchWorker::submit`, and replacement API signatures are retained. New terminal states `UnsupportedStreaming` and `RegexLimit` are incomplete and cannot prepare replacement. Regex case handling is PCRE2 Unicode caseless semantics, distinct from literal full Unicode folding.

The adapter copies a complete UTF-8 snapshot only at or below 16 MiB. Selection filters matches but retains full-document regex context. Larger subjects return `UnsupportedStreaming` before copying. No pattern-string heuristic claims streaming safety. Compilation is capped at 64 KiB input, 1 MiB compiled pattern and 250 nested parentheses. Matching uses 1,000,000 match steps per engine call, depth 1,000, heap 8 MiB, and a two-second job deadline. Non-JIT automatic callouts observe cancellation and deadline during matching; start optimizations are disabled to retain interrupt checkpoints. Byte-splitting `\C` is prohibited. FFI pointers have local RAII ownership and callout data lives through synchronous engine calls. This is locally reviewed bridge code, not a claim of independent security audit.

Compatibility catalog tested on complete bounded subjects:

| Supported | Evidence |
|---|---|
| Lookbehind and selection exterior context | `(?<=x)(ab)\b`, selection starting after x |
| Subject and line anchors | `\A`, `\z`, multiline `^`/`$` |
| Backreferences and end-sensitive alternatives | `(ab) \1`, `a.*\z|a` |
| Multiline text and contiguous anchor | newline match, `\Ga` |
| Empty matches | Advance one Unicode scalar, including final EOF match; no same-offset nonempty retry |
| Numeric replacement captures | `$0`, `$1`, `${1}`, `\1` |
| Named replacement captures | `${name}`, `$+{name}` |
| Replacement escapes | `$$`, `\\`, `\n`, `\r`, `\t` |

Capture text ranges and group-name metadata count against the result budget. Missing capture references or unsupported replacement syntax fail before mutation. Captures expand on the worker, with staged payload plus inverse-size accounting capped before each append. Current-document changes use the existing single transaction. All terminal incomplete results disable replacement. A discovered streaming-tail issue was fixed: retained hits below the final 128-entry batch are emitted even when the next hit exceeds the result budget.

Windows headless evidence, 2026-09-06:
- `cargo test -p bareline-search --lib`: 13 passed, including five regex tests (catalog, capture preparation, unsupported/invalid/resource rejection, interrupt callback, capped tail batches) and eight existing literal/extended/worker tests.
- `cargo clippy -p bareline-search --all-targets -- -D warnings`: passed.
- `rustfmt --edition 2024 crates/search/src/lib.rs crates/search/src/regex.rs`: touched files formatted; no global formatting.

Remaining backend gaps: partial-match streaming across >16 MiB subjects (therefore the required multi-line chunk-boundary acceptance remains unfulfilled), paged ByteSource scanning, folder/open-document batch orchestration and result excerpts, independent total count beyond retained result cap, aggregate client budgets, disk replacement staging and linked open-document transactions. Unsupported replacement dialects (case conversions and other Notepad++ syntax) fail explicitly. Lock-only UI validation belongs in the root unlock checklist; no desktop automation was used here.

Graft used for API/caller lookup before changes: approximately 10,541 tokens saved. Two scoped lookups failed because the graph uses the parent `Bareline-Editor/` prefix. Freshness reported no committed manifest and stale graph/code state; no graph rebuild or commit performed.
