# Bareline {{VERSION}} migration notes

Supported upgrade origins: {{VERSIONS}}. Settings/session/recovery compatibility: {{FORMAT_VERSIONS_AND_EVIDENCE}}.

## Upgrade and rollback

{{INSTALLER_PORTABLE_UPDATE_STEPS_AND_ROLLBACK_LIMITS}}
Retained updater generations are preserved; describe any manually requested cleanup separately. Do not promise recovery beyond verified cases.

## Notepad++ import

Use Review Notepad++ Import, inspect the mapping report, then Apply Reviewed Notepad++ Import. Preferences, supported shortcuts and UDL data use normal validation. File candidates require the separate Open Imported Local Files action. Source XML is not changed. Native plugins and executable macros are not imported; remote/device paths and unsafe XML are rejected. Review again after changing the source file.

## Known compatibility changes

{{CHANGE_IMPACT_WORKAROUND_AND_TRACKING_ISSUE_OR_EXPLICIT_NONE}}