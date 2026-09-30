# Visual baselines

Reviewed whole-window render baselines: `light-` and `dark-` cells at 100, 150
and 200% (`light-100pct.png` … `dark-200pct.png`), with `manifest.json` recording
each file's SHA-256, the reviewer and the comparison tolerance.

**Review required.** A baseline is a reviewed expectation, not a snapshot to
refresh until the check passes. Every change to a PNG or to `manifest.json`
needs a reviewer who has looked at the old and new images and can say why the
rendering changed. Do not regenerate baselines to hide an unexplained diff.

## What a cell contains

`apps/bareline/src/windows_app/visual_baselines.rs` composes a frame the way the
editor does — shell chrome and tab strip, the editor layer with a Rust fixture,
an open Find bar and scroll bars, and the status footer — and renders it offscreen
with the production Direct2D/DirectWrite renderer onto a WIC bitmap. The toolbar
(hidden by default), side panels and overlays need a live event loop and are not
part of a cell. Pixels depend on the system fonts, so the reference is the
GitHub-hosted `windows-2022` image.

## Checking and regenerating

```powershell
./tests/visual/capture.ps1 -Report target/visual-report     # capture and compare
./tests/visual/regenerate.ps1 -Reviewer "Your Name"         # capture and promote, then review
./tests/visual/regenerate.ps1 -Reviewer "Your Name" -Capture <downloaded capture directory>
```

A pixel differs when any channel moves by more than 16; a cell fails when more
than 0.2% of its pixels differ (`tests/visual/compare.py`). The comparator writes
`report.json` and a `<cell>-diff.png` (differing pixels in magenta) for each
failing cell. The non-required [native-journeys workflow](../../../.github/workflows/native-journeys.yml)
runs the check and uploads the capture and report. Until the first reviewed
baselines are committed it warns instead of failing.
