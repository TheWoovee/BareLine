# SPDX-License-Identifier: MPL-2.0
# Shared layout authority for final inventory generation and verification.
function Get-RequiredReleaseFiles([string]$Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Release version must be x.y.z' }
    @("bareline-$Version-windows-x64-setup.exe", "bareline-$Version-windows-x64-portable.zip", 'bareline-exthost-x64.exe', 'SBOM.json', 'LICENSE', 'SDK-LICENSES.md', 'THIRD-PARTY-NOTICES.md', 'RELEASE-NOTES.md', 'MIGRATION-NOTES.md', 'KNOWN-ISSUES.md')
}