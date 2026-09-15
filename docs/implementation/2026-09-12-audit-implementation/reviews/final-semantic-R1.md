# Combined semantic candidate review R1 — not installed

Candidate: `target/qualification/final-native-semantic-candidate.json`, captured by `final-semantic-candidate` at snapshot `16e763043b000d09e1b0650bd857785c4b5cfa13`. Capture intentionally fails after writing the candidate; it is not a passing golden test. Complete structural delta retained at `target/qualification/final-native-semantic-delta.json`: 25 changed cases of 97.

Expected changes include U11 shared dock tabs and reserved panel composition, U08 independent pane identities/text providers instead of one generic editor, U12 checkbox semantics for toggles, U10 unavailable Revert when the opening values are unchanged, U02 informational comparison status, and U05 explicit discovery status with rows shifted below it. Modal background controls remain inert.

Returned to implementors before accepting the golden:

- Recovery discard confirmation defaults to an absent Restore action, causing root fallback focus. The default must be a visible enabled confirmation/Close action. The recovery fixture must represent a writable local root and assign preview text after row rebuilding; current lost preview/disabled-discard changes reduce coverage.
- Comparison pane provider rectangles extend to y776 despite a bottom dock starting at y521. Investigate the production versus fixture layout path, preserve correct independent pane semantics, and assert viewport/provider bounds respect the reserved dock. Also validate changed comparison focus targets against actual visible commands.

No baseline was overwritten or blindly blessed. A new candidate and focused comparison follow corrected source; final native verification remains independent.

## Corrected candidate R2 accepted

At32b04b53ad79838a7925c5f6b038094b368684b1, U05 controller12 and U11 exact viewport1 pass. Candidate-r2 changes17 cases relative to R1 in only5 unique sets: preview text restored across composed/recovery fixtures; Confirm is enabled and focused100505; writable Discard/Delete semantics restored; both comparison panes end at516 (reserved dock boundary before tab padding); explicit theme focus50784 restored. Every compare text-view payload matches the reviewed dual-provider Unicode fixture exactly. Existing compare.colors/options focus50320 and compare.value59999 remain; initial compare.open/populated now focus50512 Change left source, the intended U11 dock command. Informational comparison notices use Status rather than Alert. Expected shared dock, toggle Checkbox, Settings Revert-unavailable and recovery loading/status deltas are accepted. No duplicate/rounded provider identity, lost preview coverage, hidden default action or overlapping pane bound is accepted. Final native UI evidence is still separate.

Installed the exact raw R2 JSON bytes as the reviewed golden. Both intentionally failing capture runs and the old/new structured deltas remain retained. The normal golden assertion and full integrated correctness suite must pass without capture enabled.
