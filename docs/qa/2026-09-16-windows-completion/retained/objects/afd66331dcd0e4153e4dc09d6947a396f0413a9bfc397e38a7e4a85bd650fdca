# Typed producers and final closure

Native journey evidence retains its existing schema. `typed_producers.py` adds Rust/property/fault tests, verbose Python unittest, real-host contracts, complete final package verification, retained performance aggregation and explicitly named manual observations. Their artifacts identify source, actual host binaries, signed packages, reports or fixtures independently of the Windows editor binary.

## Capture and import

The version-1 procedure contains `id`, `producer_kind`, exact `command` argv, `checks` (`id` plus exact `selector`), `artifacts` (`role`, `kind`, repository-contained `path`), complete ten-field `environment`, and `timeout_seconds`. `schema_version` is 1. Producers are `rust_tests`, `python_tests`, `host_contract`, `packaging`, `performance`, or `manual`.

Run one procedure through an outer retained receipt:

```powershell
python .github/workflows/run_test_evidence.py --output target/qualification/typed-outer.json --timeout 300 -- python tests/e2e/typed_producers.py execute --plan target/qualification/procedure.json --output target/qualification/typed-result.json
python tests/e2e/typed_producers.py verify --result target/qualification/typed-result.json
```

The producer retains an inner receipt and binds input bytes before/after execution. The outer receipt binds the exact emitted result. Both source identities must agree. An input/procedure/source change refuses import. Empty or skipped selected tests become NOT_RUN; failed processes cannot create PASS. Tests require explicit per-test output and consistent totals; ambiguous repeated names are rejected. Use a specific package/filter for repeated Rust names instead of claiming ambiguous workspace-wide output.

Python uses `python -m unittest ... -v`. Rust uses `cargo test ...`. The package producer runs the actual `packaging/windows/verify-release.ps1` and pins every final input plus config/verifier bytes. Host capture checks actual OS/build/architecture; a Windows run cannot claim a Linux/macOS host. Manual observations contain observer, timestamp, source/environment, individual statuses and retained supporting artifact roles; they remain declarations and require a different reviewer.

Manual/performance procedures run `python tests/e2e/typed_producers.py inspect --plan <same-procedure>`. Manual artifacts include `manual_attestation` (kind report) with schema 1/kind `manual_observation`, `observer`, `observed_at_utc`, `source_identity`, `environment`, and `observations` (`id`, `status`, `observed`, `artifact_roles`). Performance artifacts include `performance_report`, `performance_provenance`, every raw trial and source-after JSON, plus measured executable bytes. The hardware field is canonical sorted compact JSON of the performance hardware record. Measurements, counts and percentiles are recomputed using the official report engine. Timing remains informational; this does not approve performance claims.

Only complete **reviewed** `typed_qualification_mappings` can import acceptance records. Each mapping carries ID, procedure ID/kind, required checks, complete scope, reviewed=true and distinct implementer/reviewer. Proposed mappings and test-fixture reviewer names are not production acceptance. `adapt --result ... --receipt ... --mapping ... --evidence-id ... --cell-id ... --output ...` revalidates all of this before writing evidence.

Matrix schema 2 supports native cells with `binary_sha256` and typed cells with `producer_kind` plus `artifact_set_sha256`. Both retain exact environment/source fields. Schema 1 native matrices remain supported. A typed unit test cannot qualify a runtime command outcome; native or complete reviewed manual observation is required.

## Retention

The evidence collector follows typed result → procedure/artifacts/inner receipt and evidence → outer receipt/mapping dependencies. After exporting and removing original caches, run:

```powershell
python tests/e2e/evidence_bundle.py verify-producers --bundle <retained-directory> --expected-sha256 <collection-manifest-digest>
```

This recomputes supported producer observations and mappings using retained objects and original path identities. It lists native results that have not been recomputed. It never asserts final semantic acceptance, independent reviewer identity, or publication approval.

## Two-stage closure

`runner.py resolve --stage prerequisites ...` resolves all required cells except the two meta-criteria AC-021-01/03. Capture that command with T09. Its report is bound to output by exact path/hash. Preserve every failure and missing cell.

After independent review, provide a schema-1 `release_acceptance_closure`: review identities/time, hashed prerequisite report/receipt and hashed `release_readiness_record`. The readiness record binds the prerequisite and parity index, lists correctness issues/dispositions, checked performance evidence, limitations and evidence-limited parity claims, and keeps `release_approved:false`.

`resolve --stage final --closure ...` rechecks the current prerequisite inputs/outcomes against the captured report, then closes only AC-021-01/03. A final report cannot be its own prerequisite; changed source, outcomes, matrix/index, self-review or an unresolved P0/P1 issue refuses closure. The default `full` mode preserves the prior non-staged interface; the explicit staged path is the release workflow.
