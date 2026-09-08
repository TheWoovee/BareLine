# PR020 bounded parser mutation corpus

Source authored at closure base `1550d70`; execution **NOT_RUN**. No parser was
changed and no application, network target or component was executed. This is a
fixed-work regression corpus, not a coverage-guided fuzz campaign or a claim of
exhaustive crash/OOM safety.

The coordinator wires `parser_corpus.rs` as xtask's `security_parser_corpus` test
target. It calls the production session decoder/encoder, UDL XML importer and JSON
registry replacement, RPC framing, and recovery writer/inspector. Each seed generates
at most 97 deterministic variants: sampled truncation, high-bit flips and delimiter
insertion. Seeds are capped at 4096 bytes and variants at 4097 bytes. Inputs are
processed sequentially. The explicit session byte-limit fixture is 8 MiB + 1 byte;
RPC malformed lengths remain subject to its production 8 MiB allocation cap, and
oversized declarations are checked with a reader that fails if payload is touched.

Successful decodes must revalidate and round-trip. Failed UDL replacement must leave
the previous definition unchanged. Session duplicate IDs, oversized titles, malformed
persisted path identity and oversized input must fail. A damaged second journal
record must preserve the first acknowledged transaction and cannot advertise a
second durable revision. Temporary storage is generated and owned by each test;
decoding session paths performs no document I/O.

`package_corpus.rs` is included only in the extension package's existing
`signed_tests` module. It reuses the existing deterministic TEST signing key and
catalog policy, rehashes and resigns each altered archive, then calls the actual
catalog/fetch/install path. This reaches the post-authentication ZIP/TOML guards.
One valid package control plus 16 truncated archives, 16 truncated manifests, six
semantic TOML mutations and three unsafe archive names are bounded below 8 KiB per
archive. Malformed metadata may fail earlier at catalog validation; no trust check
is bypassed. The component payload is inert text and is never executed.

Shared-gate commands, not executed by this author:

```
cargo test -p xtask --test security_parser_corpus --locked
cargo test -p bareline-extensions-protocol authenticated_package_mutations_reach_manifest_and_archive_guards --locked
```

Run with the coordinator's ordinary outer test deadline. No timing assertion is
used as a correctness oracle. Retain failing mutation index and parser/test name;
add minimized seeds here when a failure is fixed. Do not weaken rejection assertions
to make the corpus pass. Longer fuzzing/sanitizers, allocation instrumentation and
native fault/security tests remain separate evidence. See [issue map](NOTEPADPP_ISSUE_MAP.md)
for the exact baseline classes, related controls and uncovered portions.
