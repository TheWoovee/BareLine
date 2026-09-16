# Remote primary-branch integration — 2026-09-16

The owner requested merging the accumulated changes into the remote main branch. Remote discovery identifies `https://github.com/TheWoovee/BareLine.git`, whose existing default branch is `master`; no `main` branch exists. Integrate into the existing primary branch without renaming branches or rewriting remote history.

## Scope

Commit the accumulated Windows readiness source, packaging, native/qualification tooling, progress records and retained evidence. Keep ignored local build outputs and the installer in their existing local directories. Preserve historical failures and source identities. Product acceptance remains incomplete; this integration is not a signed production release.

## Merge checks

- Fetch the current primary branch and preserve any remote commits. The initial fetch matches local HEAD `e6c1486ff0bb997d96801331b32986d612728d4f`.
- Inspect the pending file inventory and check for accidental credentials or oversized Git objects.
- Preserve byte-exact captured evidence and signed fixture inputs using `.gitattributes`; see [evidence retention contract](../../../tests/e2e/EVIDENCE_RETENTION.md). Check staged bytes against working files and verify retained bundles before committing.
- Reuse the [optimized build and 12/12 installed native smoke steps](../../qa/2026-09-16-windows-install-smoke/README.md). No product source changes in this merge step require another full build or suite.
- Publish through a normal fast-forward push, then verify the remote branch points to the committed revision. A rejected push must be investigated, never force-pushed.
