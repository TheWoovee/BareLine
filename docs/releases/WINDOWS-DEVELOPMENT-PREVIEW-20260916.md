# Windows development preview — 2026-09-16

This is a local, unsigned 0.1.0 development build from the dirty working tree based on e6c1486ff0bb997d96801331b32986d612728d4f. It is not a production release or an approved upgrade origin. Exact binaries and source/build receipts are listed in docs/qa/2026-09-16-windows-completion/README.md.

## Changes

- User-defined languages now survive restart with bounded, validated, atomic catalog storage and unambiguous extension associations. Invalid existing catalogs are preserved and reported.
- Column editing uses shared range geometry for tabs, CJK, combining text, emoji and BiDi. The native insertion/Undo rerun remains pending after foreground interruption.
- Offline authority policy is rechecked across update/helper/catalog/runtime and queued extension operations. Configured packaging carries the authenticated bootstrap policy.
- Release and evidence tools now retain exact inputs, reject failed/tampered captures and preserve incomplete qualification visibly.

## Using the local build

Run target/release/bareline.exe directly, or add --software for the software renderer. Ordinary preview builds intentionally disable configured update/download/extension installation features. Use an isolated generated profile for qualification; native harnesses supply their own scratch directories. Do not install or overwrite a daily-use copy as part of testing.

Windows 10 build 19045 and Windows 11 build 22631+ x64 are the declared floors; only the current Windows 11 build 26200 host has fresh local evidence. Other floor/hardware/input/AT cells still need real execution. Linux/macOS qualification is deferred to the next update.

## Data and migration

No general signed upgrade/rollback compatibility is claimed for this preview. Preserve user data before a later release migration. Relocate the entire portable package together with its marker/data folder. The new native portable procedure still needs execution, including final-package and actual offline checks. UDL imports are copied into the profile's validated language catalog; explicit language overrides win over extension associations.

Notepad++ import uses Review, then Apply Reviewed Import. Opening imported local file candidates is a separate action. Source XML is preserved; native plugins and executable macros are not imported. Remote/device paths and unsafe XML are rejected by the existing import contracts.

## Release boundaries

See docs/implementation/production-readiness/KNOWN-LIMITS-20260916.md for encoding, regex completeness, recovery gaps, estimated line numbers and accessibility limits. A successful build is not the signed installation/update, physical accessibility, independent SDK-author or 72-hour reliability acceptance. There is no published performance superiority claim. Production notes/SBOM/notices/inventory must be generated and reviewed for the actual configured signed candidate.
