Describe the problem and the behavior before and after this change.

Review IDs or issues addressed (also name review IDs in commit subjects, for example `Keep resident checkpoint slots off the durable journal (REC-01)`):

Validation performed and any remaining limitations:

Dependency additions and why they are needed (if applicable):

Screenshots for visible UI changes (if applicable):

### Definition of Done

Check each item that applies, and explain any applicable item that was not done.

- [ ] An automated test covers the change and fails without it, where that is practical. Otherwise a manual case is described above.
- [ ] New tests run in CI. Any `#[ignore]`d test is named above with how it is run.
- [ ] `cargo fmt --all --check` is clean, and no entries were added to `.github/workflows/clippy-baseline.json`.
- [ ] User-visible text is readable: no `{:?}` Debug output or internal error names in the UI.
- [ ] Documentation is updated where needed: README, known issues, and an entry under **Unreleased** in `CHANGELOG.md` for user-visible changes.
- [ ] UI changes: screenshots in light and dark themes at 100% and 200% scaling, and a check of UI Automation names and roles.
- [ ] Performance changes: before and after numbers from the performance harness, measured on a release build.
- [ ] Every commit is signed off (`git commit -s`) under the [Developer Certificate of Origin](https://github.com/TheWoovee/BareLine/blob/master/CONTRIBUTING.md#developer-certificate-of-origin).
- [ ] AI assistance, if any, is disclosed with `Co-Authored-By` trailers and follows the [AI-assisted development policy](https://github.com/TheWoovee/BareLine/blob/master/docs/AI_ASSISTED_DEVELOPMENT.md).
- [ ] Changes to recovery, `lifecycle.rs`, `instance.rs`, the update or extension trust chain, workflows, or packaging are called out for the extra review in [MAINTAINERS.md](https://github.com/TheWoovee/BareLine/blob/master/MAINTAINERS.md#review-and-approval).
