# Bareline Product Blueprint

**Version:** 1.3 · **Date:** 2026-09-05

**Plain text. Full power. No weight.**

This package is a clean-slate specification for a Windows-first, open-source text/code editor that matches Notepad++'s useful core capabilities while being **measurably faster, lighter and quicker to load** on the same machine, with safer recovery, isolated extensions, signed updates, cleaner multi-editing, modern discoverability and a path to Linux/macOS.

Three owner constraints shape every document here:

1. Faster, lighter, loads better than Notepad++, proven by a published side-by-side table.
2. No developer-blocking gates: performance is measured continuously from the first PR and never blocks a merge.
3. Open source (MPL-2.0 core, MIT OR Apache-2.0 SDK), owned and distributed by the product owner, with signed releases with reproducible unsigned payloads.

## Read in this order if you are the product/controller agent

1. `07_DECISION_LOG.md` (the 46 architecture decisions; overrides everything else)
2. `00_BRAND_AND_LOGO.md`
3. `00_RESEARCH_BASELINE.md`
4. `01_REQUIREMENTS.md`
5. `02_IMPLEMENTATION.md`
6. `04_PR_DEPENDENCIES_AND_PARALLELISM.md`
7. `03_PR_TRACKER.md`
8. `05_NOTEPADPP_PARITY_MATRIX.md`
9. `06_OPEN_SOURCE_AND_DISTRIBUTION.md`
10. `08_IMAGEN_PROMPTS.md` (when producing brand or UI imagery)

## Read if you are a coding agent

Read **your assigned file under `PRs/` plus `03_PR_TRACKER.md`**, and `07_DECISION_LOG.md` if an architecture question arises. Also read the assigned FC sections and acceptance cases linked from each PR. The PR files repeat the architecture guardrails, scope, non-goals, likely ownership, implementation sequence, tests and acceptance checklist so an implementation agent does not need to scan the entire directory.

## Directory

```text
Bareline_Product_Blueprint/
  README.md
  MANIFEST.json
  00_BRAND_AND_LOGO.md
  00_RESEARCH_BASELINE.md
  01_REQUIREMENTS.md
  02_IMPLEMENTATION.md
  03_PR_TRACKER.md
  04_PR_DEPENDENCIES_AND_PARALLELISM.md
  05_NOTEPADPP_PARITY_MATRIX.md
  06_OPEN_SOURCE_AND_DISTRIBUTION.md
  07_DECISION_LOG.md
  08_IMAGEN_PROMPTS.md
  mockups/README.md
  All_Screens.png          # marketing board only, not a UI reference (ADR-30)
  Comparison.png           # marketing board only, not a UI reference (ADR-30)
  PRs/
    README.md
    PR-001_....md
    ...
    PR-027_....md
```

## Important implementation stance

- **Storage.** No Scintilla buffer and no memory mapping in v1. Files up to 256 MiB stream into memory viewport-first; larger files page on demand behind the same `ByteSource` trait (ADR-01). The valid UTF-8 text view and original byte domain are distinct; undecodable spans retain original bytes through provenance metadata, and large transcodes use disk-backed storage (FC-01/02).
- **Rendering.** Native Win32 menu bar and dialogs where they are free; custom-rendered editor, tabs and panels through Direct2D/DirectWrite with both hardware and software modes, the lighter one chosen from measurement (ADR-04, ADR-32). A headless `RecordingBackend` runs every UI test on every OS from PR-001 (ADR-07).
- **Speed by construction.** A written startup budget (ADR-33), an approved dependency list with no async runtime in the editor process (ADR-09), and `xtask perf` running nightly from the first PR (ADR-10).
- **Extensions.** No third-party code loads into `bareline.exe`. The `wasmtime` host is a separate signed download; three first-party extensions (JSON, XML, Hex) ship at v1 as the SDK reference (ADR-20, ADR-25).
- **Compare.** Built in, engine first (`crates/diff`, PR-025) so conflicts and recovery reuse it, UI later (PR-017).

## Mockups

Use [compare-dark](mockups/bareline-compare-dark.png) for five diff states and [compare-merge](mockups/bareline-compare-merge.png) for directional merge affordances. Current concepts are indexed in [mockups/README.md](mockups/README.md). Replaced images are preserved under mockups/superseded. All_Screens.png and Comparison.png are obsolete exploration boards. The filename 08_IMAGEN_PROMPTS.md is retained for compatibility; actual generation uses ChatGPT image generation with provenance recorded.

## Blueprint v1.3

This is a specification package, not an implemented application. Read [foundation contracts](09_FOUNDATION_CONTRACTS.md), [acceptance and traceability](10_ACCEPTANCE_AND_TRACEABILITY.md), and [UI interaction specification](11_UI_INTERACTION_SPEC.md). The [original review](REVIEW_2026-09-05.md) is preserved; the [revision review](REVIEW_V1.3_2026-09-05.md) records changes, scores and limitations. Implementation tracker states remain NOT_STARTED.
