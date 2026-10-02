# Accessibility checks

## Semantic golden

`native-semantic.json` is the reviewed UI Automation projection of every
surface: names, roles, values, focus, hierarchy and actions. The test
`complete_native_semantic_json_golden` in
`apps/bareline/src/windows_app/accessibility.rs` builds each case headlessly and
compares it with this file. To change it, capture a candidate with the same test:

```powershell
$env:BARELINE_CAPTURE_ACCESSIBILITY_GOLDEN = "$PWD/target/native-semantic.candidate.json"
cargo test -p bareline --locked --bin bareline complete_native_semantic_json_golden
```

The capture writes the candidate and fails on purpose. Review the full diff
against `native-semantic.json`, copy the candidate over it, and rerun the test
without the variable. Never edit the golden by hand. Every change needs a
reviewer who can say why each changed field is correct for assistive
technology.

### Planned split per surface (QA-13, not yet done)

The golden is one 1.3 MB file, so a change to one surface shows up as a diff in
a very large file. The plan is to split it per surface from the same capture
code path, so no file is edited by hand:

1. In the capture test, group the `cases` map by surface, taken from the case
   name before its first `/`, `.` or `_` (`all`, `compare`, `default`,
   `extensions`, `find`, `language`, `macros`, `palette`, `panels`, `power`,
   `recovery`, `search`, `settings`, `shortcuts`, `toolbar`, `utilities`,
   `views`). The `all_panels.*` and `all_app_panels` cases (about 400 KB)
   become `all.json`.
2. With `BARELINE_CAPTURE_ACCESSIBILITY_GOLDEN` set to a directory, write one
   `<surface>.json` candidate per group, using the same serialization and
   relative-age normalization as today.
3. Without the variable, read `tests/a11y/native-semantic/<surface>.json`,
   merge the files and compare the merged map with the actual cases, failing
   on a missing, extra or duplicate case. The failure message names the
   surfaces that differ.
4. Generate the split files once with the capture, check that their merge
   equals the current `native-semantic.json` exactly, and delete the single
   file in the same change.

This was not done in the QA-13 change: the split files must come from running
the capture, and that change did not compile or run Rust. Hand-splitting the
current file is not allowed.

## Axe.Windows scan

`axe_scan.ps1` runs the Axe.Windows CLI (pinned release 2.4.2) against the
main window of one owned editor with an isolated profile, and stops only that
process. It verifies the archive's size and SHA-256 before extracting it. The
release does not publish a digest, so a maintainer must download the archive
once, review it, and record its SHA-256 in the script. Until then the scan
refuses to run, and the non-required
[native-journeys workflow](../../.github/workflows/native-journeys.yml) reports
a warning instead. Results (`.a11ytest` files and `axe-summary.json`) are
uploaded as a workflow artifact.
