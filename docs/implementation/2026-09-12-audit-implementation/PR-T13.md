# PR-T13 — Stable Clippy debt ratchet

- Issue: TECH-016
- Baseline source: `ec0d5f488bce0f523850889969b51a9f07e2dd79`
- Scope: Clippy diagnostic normalization, structured baseline comparison, self-test fixtures, and CI wiring.

## Contract decisions

- The checker keys each occurrence by lint code, lexical enclosing module/trait/impl ownership, Rust item identity, normalized diagnostic text, and normalized local source context. Raw paths and line numbers remain evidence fields and do not define identity, so a file rename or unrelated line shift does not reset debt. Identical methods in distinct impl owners remain distinct occurrences.
- Baselines are multisets. An additional occurrence of an existing lint fails; a removed occurrence makes the baseline stale and regeneration shrinks it.
- Diagnostics outside the reviewed baseline fail without category-wide suppression. Correctness/resource lints cannot be admitted by editing a global allow list.
- Each baseline qualification is keyed by the actual platform, host target, and Rust release. Missing or duplicate qualifications fail; Windows results cannot qualify Linux or macOS. A qualification is unusable until every occurrence has a reviewed `correctness-resource` or `style-design` priority and a nonempty justification, and it records the integrated HEAD plus a framed content fingerprint of tracked and untracked source. The baseline and explicitly named generated command outputs are excluded from that source identity; file payloads are hashed in fixed 64 KiB chunks.
- Empty, truncated, malformed, compiler-error, and unsuccessful Cargo JSON streams fail before baseline comparison. A successful terminal `build-finished` event is required.
- CI captures a complete warning-level Clippy JSON stream so inherited diagnostics do not stop compilation early, then applies the occurrence ratchet as the strict gate. Raw JSON, Cargo stderr, and the ratchet result are retained for every platform job.
- Root will create and review the complete baseline only from the final integrated Clippy JSON stream. This tooling branch will not freeze the current partial debt.

## Focused checks

- Standalone checker self-tests cover a new instance in an existing category, removal, unrelated line shift, source-file rename, distinct identical methods under different impl owners, multiline ordinary/raw strings plus nested comments/chars, missing platform qualification, and incomplete/malformed/failed Cargo streams.
- `python .github/workflows/check_clippy_ratchet.py self-test`
- Root owns the final full Clippy census and ratchet check after integration.

Final integrated Windows PowerShell census and draft generation:

```powershell
New-Item -ItemType Directory -Force target | Out-Null
cargo clippy --workspace --all-targets --locked --message-format=json 1> target/clippy-census.jsonl 2> target/clippy-census.stderr.log
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$rustcDetails = rustc -vV
$hostTarget = (($rustcDetails | Where-Object { $_ -like 'host: *' }) -replace '^host: ', '').Trim()
$toolchain = (($rustcDetails | Where-Object { $_ -like 'release: *' }) -replace '^release: ', '').Trim()
python .github/workflows/check_clippy_ratchet.py write-baseline --diagnostics target/clippy-census.jsonl --baseline .github/workflows/clippy-baseline.json --source-root . --platform Windows --target $hostTarget --toolchain $toolchain
```

After reviewing every generated occurrence, set `review_status` to `reviewed`, assign one of the two priorities, add the specific justification, and run:

```powershell
python .github/workflows/check_clippy_ratchet.py check --diagnostics target/clippy-census.jsonl --baseline .github/workflows/clippy-baseline.json --source-root . --platform Windows --target $hostTarget --toolchain $toolchain
```

## Unresolved qualification

- The reviewed Windows qualification and full raw Clippy output are pending the final integrated tree. Linux and macOS qualifications remain pending their own actual CI censuses; they must not be copied from Windows. CI intentionally references the final baseline and will refuse any missing platform/target/toolchain qualification.
