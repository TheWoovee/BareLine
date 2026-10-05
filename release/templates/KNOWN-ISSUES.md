# Bareline {{VERSION}} known issues

Audited commit: {{COMMIT}}. Audit date: {{UTC_DATE}}. Reliability gate evidence: {{ISSUE_CLASS_TRACEABILITY_LINK}}.

Release is blocked by any unresolved P0/P1 data loss, corruption, arbitrary-code/update, remote credential exposure, editor-start crash or unrecoverable-session defect. Do not reduce severity merely to release.

| Issue | Severity | Affected versions/scenario | Reproduction | Impact | Workaround | Tracking issue |
| --- | --- | --- | --- | --- | --- | --- |
| {{TITLE_OR_EXPLICIT_NONE}} | {{P2_OR_P3}} | {{SCOPE}} | {{EXACT_STEPS}} | {{OBSERVED_RESULT}} | {{VERIFIED_STEPS_OR_NO_WORKAROUND}} | {{ISSUE_URL}} |

Deferred features belong in the parity matrix with the actual ADR/issue; they are not undocumented known defects. Missed performance targets belong in the published measurement report with tracking links.