# Coordinated first-party component acceptance gate

Status: queued, not executed by the first-party source lane. Run once after the
host/manager owner declares PR-016 source stable. This is a headless local host
fixture gate; it does not manipulate the desktop or perform manual GUI QA.

Prerequisites: repository Rust 1.98.1 toolchain, Windows x64/MSVC, already installed
`wasm32-wasip2` target, dependency cache, and the complete shared source batch.
The coordinator owns builds. Do not run parallel copies of this gate or silently
install missing tooling as part of the measurement.

From the repository root, the coordinator runs these commands sequentially and
retains both outputs and exit codes:

```powershell
cargo build --offline --locked --release --target wasm32-wasip2 -p bareline-json-tools -p bareline-xml-tools -p bareline-hex-view
cargo test --offline --locked --release -p bareline-extension-host --test first_party -- --ignored --nocapture --test-threads=1
```

The second command must report **four tests run and passed**, with no filter that
reduces the workload. An ordinary workspace test run reports these as ignored and
provides no evidence of component execution. The tests are:

1. `first_party_components_cross_authenticated_process_boundary`: five real
   component invocations; JSON validation/minification retains the exact large
   integer, XML validation/formatting preserves `xml:space`, and Hex displays
   original UTF-16 bytes.
2. `generated_one_gib_json_validates_and_tree_reads_only_one_page`: the parent
   generates a 1 GiB JSON object by absolute offset in at most 64 KiB replies.
   Background validation must request exactly 16,384 contiguous ranges, reach
   the full source length and return `Valid JSON`. The same source's interactive
   tree must read only 65,536 bytes, show real nodes and expose string-continuation
   state. The test prints byte/read counts and elapsed time. No 1 GiB input is
   materialized or written to disk.
3. `xml_xpath_security_and_hex_original_generation_cross_the_host`: actual
   namespace/positional XPath, descendant attributes, missing attribute empty
   result, DTD denial, and original UTF-16/dirty-text-domain separation. Missing
   original bytes must produce gaps and never a decoded-text substitution.
4. `hex_five_gib_goto_reads_only_visible_original_range`: actual Hex component
   receives a virtual 5 GiB original source, jumps above 4 GiB and reads exactly
   one 576-byte viewport-plus-overscan range. Generation and domain are asserted.

Budgets: ordinary operations retain 5 seconds / 50 million fuel. Only the
explicit background validation invocation uses the parent-selected 120-second /
200-billion-fuel policy; linear memory remains limited to 128 MiB. The fixture
parent allows two seconds of transport/exit cleanup and does not extend the guest
budget. Run serially to avoid manufacturing deadline failures through test
contention. A timeout/fuel failure is a failed acceptance result, not permission
to increase the policy or reduce the source size. Record and investigate it.

Retain source commit, toolchain/OS, component and host SHA256 hashes, commands,
actual four-test output, fixture counters and duration in PR-027 evidence. Passing
proves these locally built components and broker fixture behavior through a real
authenticated child process. It does **not** prove owner-signed catalog publication,
production runtime certificate trust, manager UI permission actions, actual paged
editor-file broker access, crash-while-editing GUI behavior, or the timed clean
new-author walkthrough. Those require their owning gates and must remain distinct.
