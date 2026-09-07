# Bareline Open Source, Governance and Distribution

**Version:** 1.2  
**Date:** 2026-09-05  
**Decisions:** ADR-34 (license), ADR-35 (signing), ADR-36 (governance), ADR-20 (extension host distribution), ADR-31 (installer), ADR-10 (performance publishing) in `07_DECISION_LOG.md`.

The product owner owns the project, the trademarks, the signing keys and the release channels, and publishes the code as open source. This document says exactly what that means so a contributor, a packager or a coding agent never has to guess.

## 1. Licensing

| Component | License | Why |
|---|---|---|
| Application, core crates, native bridges, installer scripts | **MPL-2.0** | File-level copyleft: changes to Bareline files must be published, but Bareline can be combined with anything, including proprietary extensions. Compatible with every dependency in the approved list (Lexilla HPND, PCRE2 BSD-3, wasmtime Apache-2.0 with LLVM exception). |
| `crates/extension-sdk`, `crates/extensions-protocol`, `extensions/*` | **MIT OR Apache-2.0** | Extension authors face no copyleft and may sell closed extensions. |
| Name “Bareline”, wordmark, logo, app icon | **Trademark, all rights reserved** | Forks are welcome and must rename. Documented in `TRADEMARKS.md` with permitted uses (linking, unmodified redistribution, packaging manifests). |
| Documentation | **CC BY 4.0** | |

If the owner later prefers maximum permissiveness, relicensing owner-held code may be straightforward only while the owner holds all relevant rights. Third-party code keeps its own licenses. After outside contributions, obtain the necessary permissions or use a license-compatible approach; DCO sign-off does not transfer copyright. See the Mozilla MPL FAQ linked in FC-08.

Every source file carries an SPDX header. `cargo about` generates `THIRD-PARTY-NOTICES.md` for each release. `cargo deny` blocks GPL-family and unknown licenses in the dependency graph.

## 2. Governance

- **Owner-led.** The owner is the final decision maker, holds the signing keys, and approves releases. A `MAINTAINERS.md` lists people with merge rights.
- **Decision records.** `07_DECISION_LOG.md` is copied to `docs/adr/` in the repository. New architecture decisions are pull requests against that folder before code.
- **Contributions.** Developer Certificate of Origin sign-off (`git commit -s`), enforced by a DCO check. No CLA.
- **Code of conduct.** Contributor Covenant 2.1.
- **Issue triage.** Labels: `bug`, `data-loss` (P0), `security`, `perf`, `parity`, `good first issue`, `extension`. Data-loss and security issues are triaged within two working days.
- **Coding agents.** PRs produced by agents follow the same rules as humans: one PR file from `PRs/`, tests, tracker update, DCO sign-off by the human who ran the agent.

## 3. Repository layout for the public repo

The blueprint (`00_*` to `08_*`, `PRs/`) moves into `docs/blueprint/` unchanged so the public history shows why every decision was made. Root files: `README.md`, `LICENSE`, `LICENSE-SDK`, `TRADEMARKS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md`, `CHANGELOG.md`, `deny.toml`, `rust-toolchain.toml`, `Cargo.lock`.

## 4. Continuous integration (GitHub Actions)

| Job | Runs on | Blocks merge |
|---|---|---|
| fmt, clippy `-D warnings`, `cargo deny`, `cargo audit` | Windows | yes |
| `cargo test --workspace --locked` | Windows | yes |
| Neutral-crate build and tests, `RecordingBackend` UI tests | Linux, macOS | yes |
| Architecture check (no `windows` imports in neutral crates; startup ledger) | Windows | yes |
| DCO sign-off check | all | yes |
| `xtask perf --scenario launch,idle` nightly, append to `tests/perf/results/`, open issue on >10% regression | Windows | **no** |
| Reproducible-build check (two clean builds, compare hashes) | Windows, weekly | no, reported |
| Release pipeline (tag `v*`): build, sign, checksum, SBOM, notices, GitHub Release draft, winget PR | Windows | n/a |

Performance jobs are informational by design (ADR-10). Correctness, lint, license and architecture checks are the merge gates.

## 5. Build reproducibility

- Toolchain pinned in `rust-toolchain.toml`; `Cargo.lock` committed; CI and release use `--locked`.
- Release profile: `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, symbols stripped to a separate PDB uploaded with the release.
- Build path and timestamps normalized (`--remap-path-prefix`, `SOURCE_DATE_EPOCH`).
- The weekly reproducibility job documents any remaining nondeterminism in `docs/reproducible-builds.md`.

## 6. Signing and keys (ADR-35)

Use Authenticode for Windows executables and installer, and minisign for final checksums, update metadata and extension catalog. Signing provider enrollment, publisher identity and owner custody must be confirmed before release; do not assume free-service eligibility.

FC-08 specifies signed version/channel/platform/hash/length/expiry metadata, rollback protection, trusted roots and an independent offline recovery authority. A compromised sole release key cannot safely revoke itself through its own signed metadata. Routine rotation requires old/new authorization; recovery from compromise needs independently trusted distribution.

Keep the minisign key offline. CI produces the final artifact inventory; the owner verifies it and signs on offline hardware; CI verifies returned signatures before creating a release draft. No private offline key is copied into CI. Verify expected publisher identity as well as signature validity.

## 7. Release process

Cadence remains monthly minor releases with patches as needed; channels are stable and preview.

1. Pin source, lockfile, compiler, dependencies and fixture hashes; build and compare normalized unsigned payloads.
2. Sign inner executable/helper/host binaries before packing them.
3. Assemble portable ZIP, host pack and installer; sign the installer.
4. Produce SBOM, notices, symbols, final checksums and signed-metadata inventory for these final artifact bytes.
5. Complete the offline owner-signing ceremony; CI verifies the returned signatures and publisher identities.
6. Create the release draft with measured performance and known limitations; owner approves publication.
7. Publish artifacts, then the signed channel pointer last; prepare package-manager submissions from final hashes.

Signed artifacts are not promised byte-reproducible because signatures/timestamps can differ. Compare unsigned normalized payloads and record signing separately. Failure before publication leaves the previous channel intact. See FC-08.

## 8. Distribution channels

| Channel | v1 | Notes |
|---|---|---|
| GitHub Releases | yes | primary; all artifacts and signatures |
| winget | yes | manifest generated by `xtask release` |
| Portable ZIP | yes | no registry writes, `./data/` relocation |
| Scoop, Chocolatey | community | manifests accepted in the repo under `packaging/community/` |
| Microsoft Store (MSIX) | v1.1 | ADR-31 |
| In-app updater | yes | signed manifest, staged apply after exit, rollback |

## 9. Extension catalog

A public repository `bareline-extensions` holds `catalog.json` (signed with minisign), one folder per extension with its manifest, hash and source link. Extensions are added by pull request with a short review: capabilities requested match the description, package hash matches, license declared. First-party extensions (PR-027) are the first entries. Bareline fetches the catalog only when the Discover tab is opened.

## 10. Security policy

- Private vulnerability reporting through GitHub Security Advisories; acknowledgement within two working days.
- `SECURITY.md` lists supported versions, the public signing key, the rotation procedure and the threat model summary from PR-020.
- Security fixes ship as patch releases on `stable` before public disclosure.
- The security log never contains document content (FR-027).

## 11. Privacy

No telemetry, no crash upload, no account. The update check sends the version string to the release endpoint and nothing else, and can be disabled. The extension catalog fetch is manual. Editing works offline. Runtime/extensions can be installed from verified offline bundles; expired or unverifiable metadata cannot authorize an installation.

## 12. Performance claims policy (ADR-10)

The README and website may state “faster and lighter than Notepad++” only for metrics where the published table for the current release shows it, with a link to the raw JSON and the machine specification. The comparison is run on the owner's reference machine with the exact methodology in `tests/perf/README.md`. Community members are encouraged to run `cargo xtask perf --all --compare notepadpp` and submit their results as data, not as claims.
