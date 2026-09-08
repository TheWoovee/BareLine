# PR-008 Lexilla bridge evidence

Historical test chronology below is retained. The current source/remaining-scope matrix is [PR-008.md](PR-008.md); it supersedes foundation-era statements that paged styling, UDL consumers or detection are absent.

## Delivery

The `bareline-lexilla-bridge` crate builds actual C++ Lexilla implementations with the ADR-09-approved `cc` build dependency. `IDocument` is a bounded owned UTF-8 window with all 24 accessors implemented; it is not a Scintilla editor buffer. No Windows types enter the public Rust interface. The syntax worker now owns a persistent `!Send`/`!Sync` `LexerSession`; standalone `lex` retains its one-shot interface.

The C ABI validates the 256 KiB byte quota and output capacities, catches all C++ exceptions, checks cancellation during accessor calls and applies a 67,108,864-accessor operation budget. The safe Rust interface limits keyword data to 64 KiB, rejects NUL in C strings, catches callback panics before the FFI boundary, and returns only owned styles/states/levels. Offsets remain UTF-8 bytes local to the supplied window; the syntax service owns conversion to document TextOffset and revision/range caches. No lexer pointer, document pointer, or callback is retained after a call.

Nine bundled bindings cover FC-09: cpp (C, C++, C#, Java, JavaScript, TypeScript, Go), python, rust, hypertext (HTML), css, json, xml, sql and toml. Shared C-family binding is lexical grammar coverage, not semantic parsing. Embedded HTML language keyword sets and language-specific secondary keyword sets are not yet supplied by this small bridge API. Go/C#/TypeScript-specific lexical differences remain coverage limitations of the shared cpp binding.

Initial style and previous line state are accepted for provisional window styling. They do not serialize a lexer's private state. A nonzero-origin window must not be promoted to authoritative folding/comment metadata merely because these two values exist. Full-state checkpoint reconstruction remains the syntax service's responsibility; cold bounded windows are provisional under FC-09.

## Provenance and licenses

- Lexilla 5.5.3, official source https://github.com/ScintillaOrg/lexilla, tag `rel-5-5-3`, exact commit `334b90a64b0960d98446b80a24e430eac1a4a2ee`.
- Scintilla 5.5.7 headers from https://www.scintilla.org/scintilla557.zip, archive SHA-256 `801faef6a2404ec4cbe6f22c853d9bb63fc5e18f169f302a9c69b9d6f93349dd`.
- Both upstream `License.txt` files are retained under `native/lexilla-bridge/bundled`. Their permissive HPND-style notice permits redistribution with copyright and permission notice. Vendored source headers retain upstream licensing; first-party bridge source is MPL-2.0.
- Only eight lexer translation units and the upstream lexlib support files compile. Scintilla contributes public interface headers, never its editor implementation. The 5.5.7 IDocument interface is compiled directly against the bridge and Lexilla 5.5.3.
- Per-file SHA-256 hashes of the clean bundled dependency are recorded in `BUNDLED-SHA256.txt`. Original acquisitions remain in ignored `vendor/`; build and packaging rely only on `bundled/`.

## Verification

Windows x86_64 MSVC; Visual Studio 2022 Build Tools 14.44.35207 found automatically by cc. `cargo test -p bareline-lexilla-bridge`: initial 3 tests passed, including actual styling for all nine bindings, multiline comment/string fixtures, UTF-8/CRLF/NUL/empty buffers, cancellation and invalid/oversized FFI input. A direct deterministic accessor fuzz regression was then added to exercise every IDocument method over 512 generated buffers and invalid boundary positions; final result recorded below.

This evidence does not claim physical UI verification, cross-platform execution, exhaustive coverage-guided fuzzing, semantic grammar parity, or the 1 GB first-viewport performance target.

Final focused verification: cargo test -p bareline-lexilla-bridge passed 4/4 tests (512 generated accessor probes included), 0 failures, 2026-09-06. Source-level Rust formatting applied with rustfmt edition 2024.

Final additional boundary check: cargo test -p bareline-lexilla-bridge passed 5/5, including cancellation after accessor calls 5, 30 and 100 for every bundled binding (27 in-flight cancellation cases). cargo clippy -p bareline-lexilla-bridge -- -D warnings passed. No C++ exception crossed the native ABI in these cases.

## Rolling session and native consumers (2026-09-06)

`LexerSession::advance` preserves the actual ILexer5 object across contiguous newline-bounded worker requests. Its absolute-coordinate IDocument retains previous/current byte windows, with previous styles, line states and levels. Each input remains bounded to 256 KiB; combined transient input is at most 512 KiB. Reads before retained context return `UnavailableContext`; cancellation and failures poison the session. Revision, document identity, language and noncontiguous requests reset the worker session. No opaque state is guessed from integer line states. Native fallback checkpoints are emitted every 256 logical lines and at request endpoints, and the styling consumer retains at most 256 restart points.

Actual Lexilla fold levels now drive the neutral fold conversion, including Python indentation. Truncated open headers are withheld until a closing level is present. The editor consumes known folds through visual/logical row mapping for rendering, pointer hit testing and scroll extents; gutter controls and clone-specific collapse state operate on known metadata. Native language/completion dispatch and revision-checked acceptance are delivered in the language child adapter. Paged full-document completion/folding remains explicitly unavailable pending document-offset adaptation.

Focused tests: bridge 10/10 and syntax 10/10 pass; editor/app check passes. Session regressions compare rolling absolute C++ output to one-pass output, require cancellation poisoning, and verify unavailable lookbehind fails conservatively. All nine session bindings style and cancel through absolute accessors. Python folding regression excludes following unindented statements; truncated fold and quota regressions pass. No physical UI result is claimed here.

The subsequent ForwardLexer/FoldAccumulator pass carries open Lexilla headers across bounded windows; the language worker streams discovered folds through a one-slot channel, ending at EOF, cancellation or the 8192-fold limit. A cross-window fixture verifies a header closes in a later request and sparse 256-line checkpoints are emitted. Session fold ranges are explicitly half-open logical line ranges. Pending restore only collapses matching verified fold ranges; secondary views copy metadata while preserving their own collapse state.

Collapsed folds now retain exact byte anchors through submitted edit transactions, and apply their mapping only after successful document acknowledgments. Undo/redo and grouped history retain before/after anchors. Derived metadata is cleared until the mapped line range matches newly verified folds; no stale fold is displayed. Focused acknowledged edit/undo/redo and view-local pending-restore regressions pass.

The former remaining-scope table has moved to the current audit in [PR-008.md](PR-008.md). Paged syntax, verified projection, UDL editing/export, persistent lexer choice and bounded detection now have actual consumers. Paged arbitrary-size fold hiding and paged fold edit-anchor preservation remain explicit source limitations there.
