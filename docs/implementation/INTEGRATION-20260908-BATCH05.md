# Batch 05 extension closure checkpoint

This supersedes the unresolved extension gate in the [3b51881 work-in-progress checkpoint](INTEGRATION-20260908-BATCH05-WIP.md). PR016 and PR027 are implementation complete pending acceptance. The tracker now records twelve such packages, fifteen in progress and zero fully accepted. PR024 baseline review remains separate and is not promoted.

## Verified result

The coordinated 25-package offline locked gate with `bareline/perf-spans,bareline/qa-inventory`, followed by the exact search repair, established 378 effective passing Rust tests. Two additional JSON boundary regressions, one authenticated-pipe regression and four actual component tests bring the distinct passing coverage to **385**. Repeated focused passes and four supervised Windows child reruns are not added again. App accessibility six and macro four tests passed after the production composition/output ingestion changes. Native eighteen and app seventy-eight baseline tests passed. The later backend command passed 45 tests; it included the new pipe test but did not enable the original gate's offscreen renderer test, which had already passed.

Seven pure Python performance-contract tests, six source-only journey-runner tests and the architecture guard with negative fixtures passed. No desktop QA or benchmark adapter was executed.

The actual command `cargo test --offline --locked --release -p bareline-extension-host --test first_party -- --ignored --nocapture --test-threads=1` passed **4/4, zero failed or ignored**, in 96.00 seconds. It exercised five authenticated component commands, generated 1 GiB JSON validation plus bounded tree preview, XML XPath/DTD/raw-generation behavior, and a 5 GiB Hex viewport. JSON validation processed 1,073,741,824 bytes through 16,384 requests of at most 65,536 bytes in 88.947 seconds. The 120-second/200-billion-fuel background limit, 128 MiB memory limit and fixtures were unchanged.

Earlier failures exposed a nonblocking pipe write larger than its quota and then a later Wasm parser exit. The reviewed pipe fix offers at most 16 KiB per OS write while preserving framing, progress and absolute deadlines. Its real authenticated regression verifies encoded 64 KiB and 128 KiB replies. The reviewed JSON optimization consumes safe ASCII spans within the current borrowed buffer, retaining strict special-byte/UTF-8 handling and exact positions. Nine native JSON tests passed, including the two new boundary regressions. The earlier trap's cause was not confirmed; it is not relabeled as fuel exhaustion.

`cargo build -p bareline --offline --locked` passed in 26.03 seconds, verifying the default executable and embedded Windows manifest. Twelve warning groups remain for unfinished consumers and legacy methods; none were suppressed. Native SHA-256: `18B39BD795920B33C05FDDAAA74628FE3D6F627AF5B782BCB947C6593455B903`.

Tested release artifacts:

| Artifact | SHA-256 |
| --- | --- |
| Extension host | `51648CDB08358FCFB0D48A7B6FA3C42FDAE1402683BF82CC0924941D4D8A1D81` |
| JSON component | `7A5AB3D53726140F8B97D20DD29C9F1D0651150C6F9C82CD7D7F7574F288F1ED` |
| XML component | `C007CA3692DC19683DEC8B4449571145F8AB9FA3059458F0722F3940D7D48441` |
| Hex component | `0AC59D23B48B335DAD8CFA81936376DF2CB2FE3B43991A9612474D1AA0F19C02` |

These are local provenance hashes, not production trust pins. Logs remain under ignored `target/integration-batch-05*.log`, especially `components-ascii`, `json-repair`, `native-platform`, `app-accessibility`, `app-macros` and `default-build`.

## Remaining work

PR024's headless full-tree candidate passed hierarchy checks after real Compare and Extension identity repairs, but independent semantic review found fixture setup issues. It is not an accepted baseline, and current accessibility fixture edits are excluded from this completion commit. PR007/PR010 continue in an isolated sibling worktree based on 3b51881. Other documented source gaps remain open.

Native visual/input/assistive-technology acceptance, signed production distribution and other external release evidence remain pending. No remote operation occurred.
