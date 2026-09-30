# AI-assisted development policy

Much of Bareline's code, tests, and documentation has been written with AI coding agents, directed by the maintainer. This policy describes how agents are used, what review AI-assisted changes need, how their provenance is recorded, and what data agents may handle. It applies to the maintainer and to every contributor who uses AI tools, whatever the tool.

AI assistance does not lower the bar for a change. Every rule in [CONTRIBUTING.md](../CONTRIBUTING.md) and the Definition of Done in the pull request template applies in full.

## How agents are used

- **Scoped tasks.** An agent works on one defined task at a time, such as a review finding identified by its ID (for example `REC-01`), a failing test, or a documentation update. The task states the problem, the files likely involved, and the test that must pass.
- **Isolated branches.** Each agent works on its own branch, in its own working tree. Agents do not push to `master`, merge pull requests, create or move release tags, or change repository settings, branch rules, secrets, or signing configuration.
- **Integration.** Branches from parallel agents are merged into an integration branch, which is then built and tested as a whole before anything reaches `master` through a pull request with the required checks.
- **Human direction.** A person decides what work is done, reviews the result, and decides whether it is merged. Product, security, licensing, and release decisions are made by the maintainer, not by an agent.

## Review requirements

- **The submitter is accountable.** The person who opens a pull request must have read and understood the whole diff, and answers for it as if they wrote it by hand.
- **Independent review.** Every AI-assisted change is reviewed by someone other than the agent session that wrote it. Where possible this is a human maintainer other than the submitter. While Bareline has one maintainer, a maintainer-authored change is reviewed by the maintainer and by a separate agent review session that did not take part in writing it, and the pull request says so. This is not independent human review, and the project does not claim that it is (see [MAINTAINERS.md](../MAINTAINERS.md#review-and-approval)).
- **No self-approval.** An agent must not approve, merge, or mark as verified its own work, and an agent review never counts as a required approval. A reviewing agent reports findings; a person decides.
- **Tests with every fix.** A fix includes an automated test that fails without the fix, where that is practical, and runs in CI. Otherwise the pull request describes a manual case and why automation was not practical.
- **Do not weaken checks.** Agents must not delete, skip, `#[ignore]`, or loosen existing tests, lower thresholds, add Clippy baseline entries, or regenerate golden files to make a change pass. A golden-file or baseline update is its own commit with an explanation of what changed and why it is correct.
- **Report what was actually run.** Validation notes list the commands that were run and their results. A check that was not run is reported as not run; an agent must not describe untested behavior as verified.
- **High-risk areas.** Changes to the areas listed in [MAINTAINERS.md](../MAINTAINERS.md#high-risk-areas), such as recovery, file lifecycle, instance handoff, update and extension trust, workflows, and packaging, are called out in the pull request and get the extra review described there.
- **Dependencies.** An agent does not add a dependency unless the person directing the task agrees, and the pull request explains the need, license, and build and runtime cost as CONTRIBUTING requires.

## Provenance

- **Commit trailers.** A commit whose content was substantially written by an AI tool ends with a `Co-Authored-By` trailer naming the tool and model, for example:

  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```

- **Sign-off.** Only a person can certify the [Developer Certificate of Origin](../CONTRIBUTING.md#developer-certificate-of-origin). The person who submits AI-assisted work adds the `Signed-off-by` trailer; an agent must not add one on a person's behalf.
- **Pull request disclosure.** The pull request says which parts were AI-assisted and what the submitter checked by hand.
- **Traceability.** Commit subjects name the review IDs or issues they address, so each change can be traced to the finding, test, and review that justified it.
- **History.** Earlier commits, including the initial public source, do not consistently carry `Co-Authored-By` trailers even where agents were used. This policy applies to new commits; history is not being rewritten.

## Data handling

- **No secrets.** Never give an agent signing keys, certificates, tokens, passwords, or production trust configuration, and never let it write them into the repository. Agents work with the checked-in test fixtures only.
- **No user data.** Do not give an agent documents, recovery journals, session files, diagnostics, or screenshots that a user shared, unless the user agreed to it. Reproduce problems with synthetic files instead.
- **Embargoed security reports.** Share details of an unfixed vulnerability with an AI service only when its terms do not allow retaining or training on the content, and only as much as the fix needs. Follow [SECURITY.md](../SECURITY.md) for disclosure.
- **Local effects.** Agents run tests against temporary or disposable files and profiles, never against a person's real documents or Bareline profile.
- **Licensing.** Do not ask an agent to reproduce third-party code, and do not accept output that appears to copy it. Generated code must be compatible with the file's license; keep SPDX headers, and preserve attribution for any third-party code exactly as for hand-written changes.

## Contributors using AI tools

Outside contributors may use AI tools under the same rules: disclose the assistance, sign off personally, include tests, and be ready to explain every line in review. A pull request whose author cannot explain or support its changes may be closed.

> **TODO (owner):** review this policy, and revisit the review requirements when a second maintainer joins.
