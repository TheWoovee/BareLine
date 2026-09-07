# Mockup review notes

## Batch 1, 2026-09-05, 13 UI screens at 1672×941 (16:9)

Scores use the ten-point checklist in `../08_IMAGEN_PROMPTS.md` section 5. "Fix" means a Figma text-layer fix on the existing image; "Regen" means regenerate with the updated prefix.

| ID | File | Score | Verdict | Fixes needed |
|---|---|---|---|---|
| A1 | bareline-main-editor-dark.png | 9 | Ship after fix | Stray `\|` glyph before the caret on line 12. Logo is a plain `b`; final vector mark replaces it. |
| A2 | bareline-main-editor-light.png | 9 | Ship after fix | Hint text sits in the status bar instead of the editor corner. Code differs from A1; acceptable. |
| B | bareline-workspace-split.png | 7 | Fix or regen | Menu bar is VS Code's (File Edit Selection View Go Run Help). Sample TOML contains `[lsp]`, `inlay_hints`, `auto_save`; log mentions "language servers" and "loading plugins". Line 28 repeated. |
| C1 | bareline-find-bar.png | 7 | Fix | No menu bar. Tabs are not the standard six, no pin. Status bar is not the six-segment spec and lacks INS and size. Line 26 repeated. |
| C2 | bareline-search-panel.png | 7.5 | Fix | No menu bar. Three tabs. Line 42 repeated. Status bar shows both "Rust" and "Plain Text" and "CRLF"; replace with the six-segment spec. Panel and results are on spec. |
| D1 | bareline-settings.png | 9 | Ship after fix | No menu bar. Status bar language shows "Plain Text" while left reads "Settings"; pick one. Rows, keys, search and Advanced (12) are exact. |
| D2 | bareline-extensions.png | 9 | Ship after fix | No menu bar, no status bar. Tabs, banner, chips, runtime line are exact. |
| E | bareline-huge-log.png | 9 | Ship after fix | No menu bar. Caret not clearly visible. Everything else exact: streaming counter, indexing status, progress line, no read-only, no large-file banner. |
| F1 | bareline-compare-dark.png | 8.5 | Fix | No menu bar. Right pane line 18 repeated; rows 31 to 37 crowd on both sides. Sample TOML contains `[plugins]` and `telemetry = true`. Intraline emphasis on changed lines is faint. All five states, glyphs, counter, spacer and overview strip are correct. |
| F2 | bareline-compare-options.png | 9 | Ship after fix | No menu bar, no status bar. Moved block behind the popover shows a `+` glyph; should be `↕`. Popover is exact. |
| G | bareline-recovery-center.png | 8.5 | Fix | No menu bar. Recovered documents already appear as open tabs behind the panel; the panel should precede opening. Status bar shows both "Markdown" and "Plain Text". Close `×` sits left of the tab name. |
| H | bareline-command-palette.png | 8 | Fix | No menu bar, no status bar. Six tabs but wrong names, no pin. Sample code uses `#[tokio::main]`, which contradicts the dependency policy; replace with std code. Palette itself is exact. |
| I | bareline-tail-mode.png | 9 | Ship after fix | No menu bar. Line numbers lack thousands separators. Banner, lock icon and status bar exact. |

## Systematic findings

1. **Menu bar missing on 11 of 13 screens.** Cause: the style prefix did not demand it; only A1, A2 and B named it. The prefix now forces a menu bar and a status bar on every screen. Regenerate B, C1, C2, H with the new prefix; add the menu bar in Figma to the others.
2. **Repeated line numbers** in B, C1, C2, F1. Known Imagen behavior; fix as text layers.
3. **Out-of-scope words in sample content**: `[lsp]`, `auto_save`, `plugins`, `telemetry`, `language server`, `tokio`. Replace in Figma or regenerate. The prompt file now lists these as forbidden sample text.
4. **Status bars drift** on C1, C2, D1, G. Standardize to the six segments: language, size · lines, Ln/Col, EOL, encoding, INS.
5. **Accent, palette, density and chrome are consistent** across all 13 screens and match the tokens. No green, no leaf, no toolbar, no dialogs, no "Large File Mode", no "Plugins".

## Batch 2, 2026-09-05, five regenerated screens with the corrected prefix

Originals moved to `superseded/*-v1.png`. All five new versions replaced the originals.

| ID | File | Old | New | Verdict | Remaining fixes |
|---|---|---|---|---|---|
| B | bareline-workspace-split.png | 7 | 8.5 | Keep new | Line numbers 37 and 38 render as 33. Sample text still has `auto_save`, "build finished", "reloading diagnostics"; replace with neutral config and log lines. |
| C1 | bareline-find-bar.png | 7 | 9.5 | Keep new | None required. Menu bar, six tabs with pin, six-segment status bar and sequential line numbers are all exact. |
| C2 | bareline-search-panel.png | 7.5 | 9.5 | Keep new | None required. |
| F1 | bareline-compare-dark.png | 8.5 | 9 | Keep new | Menu bar still missing; add in Figma. All five states, spacer, overview strip, counter and status bar are exact. Sample text is clean. |
| H | bareline-command-palette.png | 8 | 8.5 | Keep new | No status bar; add in Figma. Line 23 and line 25 each repeat. Tabs and menu bar now match A1. |

## Brand batch, 2026-09-05, eight images in `logo-brand/`

| ID | File | Score | Verdict | Notes |
|---|---|---|---|---|
| L1 | bareline-logo-mark-light.png | 8 | Use as concept | Monoline `b`, folded corner, teal notch. No text lines inside the bowl. |
| L1b | bareline-logo-mark-dark.png | 8 | Use as concept | Same construction as L1; consistent pair. |
| L1c | bareline-logo-exploration.png | 8.5 | Reference | Clean 3x3 sheet varying stroke weight and radius. Middle column, middle row is the best balance for 16 px legibility. |
| L2 | bareline-app-icon.png | 8.5 | Use as concept | Adds two text lines inside the bowl, which matches the brand doc better than L1. Smallest tile correctly drops fold and notch. |
| L2b | bareline-app-icon-light.png | 8 | Use as concept | Same mark as L2 on the light tile. Fold reads as a cut into the bowl; refine in vector. |
| L3 | bareline-wordmark.png | 8.5 | Use as concept | Mark with text lines, geometric sans wordmark, tagline in muted gray. Type is close to the intent. |
| L3b | bareline-wordmark-dark.png | 8 | Use as concept | Mark without text lines, so it does not match L3. Pick one mark before vector. |
| L4 | bareline-hero.png | 6 | Regenerate | macOS traffic lights and a macOS menu set on a Windows product; line numbers 14, 16, 15, 18; `eprintln(!` typo; mark differs from every other asset. |

**Decision needed on the mark.** Two variants appeared: plain monoline `b` with fold and notch (L1, L1b, L3b, L1c) and the same `b` with two text lines inside the bowl (L2, L2b, L3). The brand doc describes the bowl as two text lines, so the L2/L3 variant is the closer match and the recommended master. Whichever is chosen, redraw once as vector on the 24-unit grid and derive every asset from it; do not regenerate marks per asset.

## Not yet final

The vector logo. The `b` in every title bar is a placeholder until the vector mark exists. The hero image needs one more run with "Windows 11 window chrome with minimize, maximize and close at the top right; no macOS traffic lights" added to the prompt.
