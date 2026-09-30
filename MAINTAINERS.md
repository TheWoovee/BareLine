# Maintainers

## Current maintainers

| Maintainer | Areas | Responsibilities |
| --- | --- | --- |
| [TheWoovee](https://github.com/TheWoovee) | All areas | Review and merge, issue triage, security response, releases, and the proposed signing approver role described in [CODE_SIGNING.md](CODE_SIGNING.md). |

Bareline currently has **one maintainer**. Every path in [CODEOWNERS](.github/CODEOWNERS) is owned by that maintainer, and there is no backup for security response, releases, or signing approval. Reports and reviews therefore depend on one person's availability.

> **TODO (owner):** recruit at least one additional maintainer, then add them here and in `.github/CODEOWNERS`, and name a backup for security response in [SECURITY.md](SECURITY.md).

## Review and approval

All changes to `master` go through a pull request with passing required checks and resolved review conversations, as described in [CONTRIBUTING.md](CONTRIBUTING.md#submit-a-pull-request). The repository currently requires **zero** approvals, because the only maintainer cannot approve their own pull requests. Maintainer-authored changes therefore do not receive independent human review today; the [AI-assisted development policy](docs/AI_ASSISTED_DEVELOPMENT.md) describes the review they do receive.

The target, once a second maintainer is available, is:

- At least **one approval** from a maintainer other than the author for every pull request.
- **Two reviewers** for changes to the high-risk areas below.
- No one approves their own change, and no AI agent counts as an approver.

> **TODO (owner):** raise the required approval count in the [default-branch rules](https://github.com/TheWoovee/BareLine/rules/24131701) to one, and enable "Require review from Code Owners", when a second maintainer joins.

### High-risk areas

These areas can lose user data or weaken a trust boundary. Call out changes to them in the pull request, and include a behavioral test for the failure being fixed:

- Recovery journals, checkpoints and sessions: `crates/file-io/src/recovery/`, `crates/file-io/src/*recovery*.rs`, `crates/file-io/src/session/`, and the application's `recovery.rs` and `session.rs`.
- File lifecycle: `crates/file-io/src/lifecycle.rs` and `apps/bareline/src/windows_app/lifecycle.rs`.
- Instance handoff: `instance.rs` in `crates/platform-windows` and in the application.
- Distribution and updates: `crates/distribution`, `apps/update-helper`, and the update modules in `crates/platform-windows` and the application.
- Process launch, the extension host and its transport, path trust, and shell integration in `crates/platform-windows`.
- CI workflows, packaging, release configuration, and signing inputs: `.github/`, `packaging/`, `release/`, `scripts/`, and `build-support/`. See the [code signing policy](CODE_SIGNING.md) before changing them.

## Triage

New issues are labeled `needs-triage` by the issue forms. The maintainer reviews them, reproduces what can be reproduced, and applies area and priority labels. Issues from the **Data loss or crash** form are labeled `data-loss` and `P0` and are triaged before other work.

> **TODO (owner):** create the `needs-triage`, `data-loss`, `P0`, and `parity` labels used by the issue forms. GitHub skips a form label that does not exist in the repository.

## Becoming a maintainer

A contributor can be invited to become a maintainer after a sustained record of reviewed contributions, careful review of others' changes, and familiarity with the high-risk areas above. The existing maintainers decide by consensus. Maintainers must use multi-factor authentication on GitHub and follow the [code of conduct](CODE_OF_CONDUCT.md), [security policy](SECURITY.md), and [code signing policy](CODE_SIGNING.md).

A maintainer who can no longer take part should say so, so that their areas and CODEOWNERS entries can be reassigned.
