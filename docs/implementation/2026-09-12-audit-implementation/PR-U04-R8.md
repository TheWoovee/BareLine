# PR-U04 R8 — Clarify folder-search completion summary

Baseline: `ab3e4ce1adb7c93d31810c1bc07a2f07c6549d65`

## Diagnosis

The folder-search footer labels `summary.searched_files` as files containing matches. The field counts every successfully scanned file, including files with zero matches. The R6 fixture therefore showed `3 matches in 3 files` although only two files contained the three matches.

## Change

- `crates/app/src/search_panel.rs`: label the existing totals explicitly as matches, files searched, files skipped, and completeness.

The copy continues to use `searched_files`; it does not derive a matching-file count from retained result groups, which may be truncated by result limits.

## Evidence and acceptance

- Source evidence: `target/qualification/final-native-audit/evidence/r6-folder-search-results.json` records three matches, three scanned files, zero skipped files, and two visible matching-file groups.
- Safety evidence: `target/qualification/final-native-audit/evidence/r6-folder-reviewed-replace-pass.json` confirms the reviewed three-match/two-file apply and rollback path.
- Worker verification is limited to exact-file formatting and `git diff --check`; no Cargo, build, or native journey is run for this copy-only correction.
- Native acceptance: the same R6 fixture reads `3 matches; 3 files searched; 0 skipped; Complete` (or equivalent explicit wording), without implying that all scanned files matched.
