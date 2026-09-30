# Code signing policy

## Current status

Bareline's Windows preview downloads are **unsigned**. A free open-source code-signing application is being prepared; it has **not been submitted or approved**. No signing service or certificate is configured for public releases, and Bareline does not currently claim SignPath sponsorship or certification. SHA-256 checksums help compare downloaded bytes but are not code signatures.

Preparation follows the published [SignPath Foundation conditions](https://signpath.org/terms.html). Acceptance and any provider-specific requirements remain the provider's decision. Provider credit will be added only after approval and confirmation of the service actually supplied. Signing does not change the preview's qualification status.

## Responsibilities and source controls

| Role | Responsible person |
| --- | --- |
| Maintainer and author | [TheWoovee](https://github.com/TheWoovee) |
| Reviewer of outside contributions | [TheWoovee](https://github.com/TheWoovee) |
| Proposed signing approver, if signing is approved and configured | [TheWoovee](https://github.com/TheWoovee) |

Bareline currently has one maintainer. These roles do not represent independent reviewers; maintainer-authored changes do not currently receive a mandatory second-person approval.

The [protected default branch](https://github.com/TheWoovee/BareLine/rules/24131701), `master`, requires pull requests, passing required checks against the current base, and resolved review conversations. It blocks force pushes and deletion. The required checks cover Windows correctness, shared-code checks on Linux and macOS, dependencies, and reproducibility. The mandatory approval count is zero to reflect the single-maintainer structure; outside contributions are reviewed by the maintainer. [Release tags matching `v*`](https://github.com/TheWoovee/BareLine/rules/24131703) are protected from rewriting and deletion.

Repository maintainers and any future signing-service users must use multi-factor authentication. Signing-service enrollment, account protections, permissions, and approval requirements must be verified before enabling signing. Private keys, certificates containing private keys, and signing credentials must stay outside the repository.

## Proposed signing process

This process is not enabled yet:

1. Select a release commit that passed the required checks and record the commit, build workflow, dependency inventory, and artifact hashes.
2. Build the release artifacts from that source using the reviewed build and packaging scripts. Identify the exact executables and installer submitted for signing; bundled third-party components retain their attribution and license notices.
3. Have the designated approver inspect the source/build identity, test results, artifact inventory, release notes, and unresolved limitations before manually approving a signing request.
4. Verify returned signatures and publisher identity, then regenerate checksums from the final signed bytes. Publish the exact source reference, artifacts, verification instructions, and accurate signing status together.

Unapproved, failed, or unavailable signing must never be represented as a signed release. The [Windows packaging guide](packaging/windows/README.md) describes current unsigned preview packaging and the separate configured-release path.

## Privacy

Signing, if approved later, will be a release-build operation, not a service that receives users' edited documents from the running editor. The [privacy policy](PRIVACY.md) describes the data Bareline keeps and its network use.

Installation and removal are described in the [README](README.md#install-and-run). Use [private vulnerability reporting](SECURITY.md) for security concerns.
