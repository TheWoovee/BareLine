# Security

Bareline is currently a Windows development preview. Security fixes target the
latest source and preview; there is no long-term support policy for older builds.

Report suspected vulnerabilities through the repository's
[private vulnerability reporting page](https://github.com/TheWoovee/BareLine/security/advisories/new).
Include the version or commit, Windows version, reproduction steps, and a small
synthetic example. Keep credentials, personal documents, and private paths out of
public issues. Use [GitHub Issues](https://github.com/TheWoovee/BareLine/issues) for
ordinary bugs that do not disclose a vulnerability.

Default preview builds disable online updates and extension installation/execution.
Public preview downloads are unsigned and do not establish a trusted update chain.
Configured distribution requires the public trust configuration and signing process
described in [the release guide](packaging/windows/CONFIGURED-RELEASE.md).
Private signing keys and certificates must never be committed.
