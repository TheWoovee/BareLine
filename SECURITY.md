# Security

Bareline is currently a Windows development preview. Security fixes target the
latest source and preview; there is no long-term support policy for older builds.

## Supported versions

| Version | Security fixes |
| --- | --- |
| Latest preview on the [Releases page](https://github.com/TheWoovee/BareLine/releases) | Yes |
| Current `master` source | Yes |
| Older previews, including `v0.1.0-preview.20260928` and `v0.1.0-preview.20260928.1` | No. Update to the latest preview. |
| Forks, modified builds, and builds with test fixture trust enabled | No |

Fixes ship in a new preview. Previews are updated manually, so check the release
notes for security fixes and install the newest preview.

## Reporting a vulnerability

Report suspected vulnerabilities through the repository's
[private vulnerability reporting page](https://github.com/TheWoovee/BareLine/security/advisories/new).
Include the version or commit (from **Help → About Bareline → Copy diagnostics**),
Windows version, reproduction steps, the impact you expect, and a small synthetic
example. Keep credentials, personal documents, and private paths out of
public issues. Use [GitHub Issues](https://github.com/TheWoovee/BareLine/issues) for
ordinary bugs that do not disclose a vulnerability.

If you cannot use private vulnerability reporting, open a public issue that asks
for a private contact and contains no details of the vulnerability.

## What to expect

- **Acknowledgement within 2 working days** of the report, as a reply on the
  private advisory. Working days are Monday to Friday, excluding public holidays.
- An initial assessment of validity and severity, and updates as the
  investigation and fix progress. Tell us if you need a decision by a
  particular date.
- Credit in the published advisory and release notes, unless you ask not to be
  named.

Bareline has one maintainer ([@TheWoovee](https://github.com/TheWoovee)) and no backup
for security response, so an absence can delay acknowledgement. If you receive
no acknowledgement within 2 working days, add a comment to your private report.

## Coordinated disclosure

Please keep vulnerability details private until a fix is released or 90 days
have passed since the report, whichever comes first, unless we agree on another
date. When a fix is released, we publish a GitHub security advisory describing
the affected versions, impact, and fixed version, and request a CVE through
GitHub where appropriate. If a vulnerability is already public or actively
exploited, we may publish guidance before a fix is available.

> **TODO (owner):** confirm the 90-day default disclosure window.

## Scope

In scope:

- The editor and the other code in this repository, including file loading and
  saving, encodings, recovery journals and sessions, search and regular
  expressions, clipboard handling, external tool launching, instance handoff,
  and shell integration.
- The published preview installer and portable ZIP, including installation,
  uninstallation, and the files and registrations they create.
- The update helper, the extension host, the extension SDK, and their trust
  checks, even though online updates and extension execution are disabled in the
  default preview.
- CI workflows, packaging, release configuration, and signing inputs in this
  repository.

Out of scope:

- Vulnerabilities in third-party components that Bareline does not reach in a
  way that exposes users. Report those upstream, and tell us if Bareline is
  affected.
- Windows warnings about the unsigned preview executables, which are documented
  in the [code signing policy](CODE_SIGNING.md).
- Attacks that already require an administrator account or full control of the
  user's account.
- Slow operations or refusals within the documented resource limits. A crash,
  hang, or data loss caused by crafted input is in scope.
- The checked-in test trust fixtures, unless a public build accepts them.

## Distribution and signing

Default preview builds disable online updates and extension installation/execution.
Public preview downloads are unsigned and do not establish a trusted update chain.
Configured distribution requires the public trust configuration and the signing process
described in [CODE_SIGNING.md](CODE_SIGNING.md).
Private signing keys and certificates must never be committed.
