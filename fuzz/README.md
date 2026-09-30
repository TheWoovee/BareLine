# Coverage-guided fuzzing (QA-05)

This directory is a separate Cargo workspace for [cargo-fuzz]. The root workspace
excludes it, so `libfuzzer-sys` (and its `arbitrary` and `cc` dependencies) never enter
the product dependency graph, the root `Cargo.lock`, `cargo deny` or the SBOM.
`fuzz/Cargo.lock` was resolved from the root lockfile, so every shared crate keeps the
audited product version; only `libfuzzer-sys` and this package were added.

The deterministic regression corpora in `tests/security` and the seeded property tests
(`crates/*/tests/prop_*.rs`) run in the normal test suite. These targets are the
open-ended, coverage-guided complement and run only in the nightly workflow.

| Target | Public API under test | Checked properties |
| --- | --- | --- |
| `codec_roundtrip` | `codecs::Decoder`/`Encoder` | spans tile the input; chunk splits and sink backpressure never change the decode; exact Unicode/Latin-1 spans re-encode to their bytes and text round-trips |
| `codec_detect` | `codecs::detect` | verdict matches BOM/UTF-8/legacy evidence; decoding with the verdict tiles the input |
| `journal_replay` | `recovery::inspect`, `replay_transactions`, `recover_to` | a sealed two-record journal with a fuzzed tail, CRC-framed fuzzed records or a fuzzed manifest; the valid prefix survives and reconstructs byte-exactly |
| `settings_toml` | `SettingsDocument`, `Settings::parse`, inline editor | never panics; saved TOML reloads with identical values and diagnostics; accepted inline input reformats to the same value |
| `macro_import` | `Macro::import_toml`, `ExternalDefinition::import_toml`, placeholders, output links | accepted definitions validate and re-import losslessly; literal templates expand to themselves |
| `udl_import` | `udl::import_notepad_xml`, `udl::Registry`, `udl::lex` | accepted definitions validate, persist as JSON and lex in-bounds UTF-8 spans |
| `function_list_import` | `outline::import_function_list`, `outline::Definition` | deterministic reports; lossless TOML; ordered, in-bounds symbols |
| `diff_script` | `bareline_diff::compare`, `apply_hunk_with_policy` | hunks are an edit script from left to right; merges apply to their target |
| `regex_search` | `bareline_search::scan` (PCRE2), `prepare_replace` | ordered, aligned matches; a complete Replace All applies |

## Running locally

cargo-fuzz needs a nightly toolchain and a libFuzzer-capable host (Linux or macOS;
the nightly job uses Ubuntu). From the repository root:

```sh
cargo +nightly-2026-09-15 install --locked cargo-fuzz --version 0.13.2
cargo +nightly-2026-09-15 fuzz run codec_roundtrip fuzz/corpus/codec_roundtrip -- -max_total_time=60
```

Pass a scratch directory first (for example `target/fuzz-corpus/<target>`) and the
seed directory second to keep new inputs out of the tree. Reproduce a crash with
`cargo +nightly-2026-09-15 fuzz run <target> fuzz/artifacts/<target>/crash-...`, minimize
it with `cargo fuzz tmin`, and add the minimized input to `fuzz/corpus/<target>/` together
with the fix. `journal_replay` keeps one scratch journal under the system temporary
directory per process (`bareline-fuzz-journal-<pid>`).

## Nightly budget

`.github/workflows/fuzz-nightly.yml` builds every target once and runs each for 200
seconds (about 30 minutes for the nine targets), with a 30 second per-input timeout.
Crashes, timeouts and out-of-memory reproducers are uploaded as artifacts. The
toolchain date and cargo-fuzz version are pinned in the workflow; update both together.

## Deferred

- **Lexilla C++ bridge under AddressSanitizer.** The plan calls for building
  `native/lexilla-bridge` with `/fsanitize=address` under a libFuzzer driver. cargo-fuzz
  instruments only Rust code; the bridge's C++ is compiled by `cc` without sanitizer
  flags, so memory errors inside Lexilla are not detected here. This needs an MSVC ASan
  (or clang `-fsanitize=address,fuzzer`) build of the bridge and a native driver, and is
  tracked separately.
- Windows-only surfaces (`platform-windows`, the renderer, the update helper) are not
  fuzzed; libFuzzer targets here must stay portable.

[cargo-fuzz]: https://github.com/rust-fuzz/cargo-fuzz
