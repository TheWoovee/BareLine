# Bareline Mockup Index

**Version:** 1.2 • 2026-09-05. Current concepts, with explicit remaining limitations. Images are visual references; [interaction contracts](../11_UI_INTERACTION_SPEC.md) define actual behavior. No screenshot is a tested application.

## Revised and new concepts

Seven assets were created using **ChatGPT image generation**, not Google Imagen. Five replace existing screens and two add missing failure/preview states. Original images are preserved; no original mockup was deleted.

| Current image | Score / 10 | Superseded original | Review notes |
|---|---|---|---|
| [bareline-compare-dark.png](bareline-compare-dark.png) | 8.5 | [Archived image](superseded/bareline-compare-dark-v2.png) | Directional merge, complete menus, aligned four-line sample and unsaved target; all five diff states still require interactive verification. |
| [bareline-workspace-split.png](bareline-workspace-split.png) | 8 | [Archived image](superseded/bareline-workspace-split-v2.png) | Correct Rust outline/content and full chrome; second pane needs its own visible config.rs tab and placeholder icon needs production replacement. |
| [bareline-settings.png](bareline-settings.png) | 8.5 | [Archived image](superseded/bareline-settings-v1.png) | Point units, scope, saved-state feedback and full menu; generated title/tab arrangement and sample footer require implementation normalization. |
| [bareline-extensions.png](bareline-extensions.png) | 8.5 | [Archived image](superseded/bareline-extensions-v1.png) | Runtime removal, per-extension controls and byte-mode labels; generated publisher/version are placeholders and JSON panel chip is omitted. |
| [bareline-recovery-center.png](bareline-recovery-center.png) | 8.5 | [Archived image](superseded/bareline-recovery-center-v1.png) | Complete/edits-only/corrupt-tail states and separate recovered copy; general help should be distinguished from selected row status. |
| [bareline-source-changed.png](bareline-source-changed.png) | 8.5 | New state | Unavailable regions, journal timestamp and partial export; restore explanatory text beside the hatched-area ellipsis in implementation. |
| [bareline-replace-preview.png](bareline-replace-preview.png) | 9 | New state | Changed-since-preview exclusion and explicit eligible-file count; footer/context values remain illustrative. |

## Retained references

These images were reviewed but not regenerated in v1.2; their previous scores and limitations remain. Read the interaction spec for the now-defined failure/keyboard states.

| Image | Score / 10 | Use |
|---|---|---|
| [bareline-main-editor-dark.png](bareline-main-editor-dark.png) | 8.5 | Retained visual concept; original review corrections still apply |
| [bareline-main-editor-light.png](bareline-main-editor-light.png) | 8 | Retained visual concept; original review corrections still apply |
| [bareline-find-bar.png](bareline-find-bar.png) | 8.5 | Retained visual concept; original review corrections still apply |
| [bareline-search-panel.png](bareline-search-panel.png) | 8 | Retained visual concept; original review corrections still apply |
| [bareline-huge-log.png](bareline-huge-log.png) | 7.5 | Retained visual concept; original review corrections still apply |
| [bareline-compare-options.png](bareline-compare-options.png) | 7 | Retained visual concept; original review corrections still apply |
| [bareline-command-palette.png](bareline-command-palette.png) | 8 | Retained visual concept; original review corrections still apply |
| [bareline-tail-mode.png](bareline-tail-mode.png) | 8 | Retained visual concept; original review corrections still apply |
| [bareline-logo-mark-light.png](logo-brand/bareline-logo-mark-light.png) | 7.5 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-logo-mark-dark.png](logo-brand/bareline-logo-mark-dark.png) | 7.5 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-logo-exploration.png](logo-brand/bareline-logo-exploration.png) | 8 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-app-icon.png](logo-brand/bareline-app-icon.png) | 8 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-app-icon-light.png](logo-brand/bareline-app-icon-light.png) | 7.5 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-wordmark.png](logo-brand/bareline-wordmark.png) | 8 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-wordmark-dark.png](logo-brand/bareline-wordmark-dark.png) | 7 | Brand concept only; hero is obsolete Windows collateral |
| [bareline-hero.png](logo-brand/bareline-hero.png) | 5.5 | Brand concept only; hero is obsolete Windows collateral |

## Provenance and supersession

[Generation log](GENERATION_LOG.json) records actual tool, date, image dimensions, prompts and replacement relationships. [Prompt briefs](../08_IMAGEN_PROMPTS.md) retain the old filename for compatibility. [Review notes](NOTES.md) distinguish corrected and remaining issues. The logical viewport is 1600×900; generated files are measured at 1672×941 and are not pixel-exact layout assets.

All_Screens.png and Comparison.png remain at the root for historical links, with copies under superseded. They are obsolete exploration boards, not current release marketing. The old NOTES_v1.1 and prompt archive are historical; their ship-ready/Figma/Imagen claims are not current provenance or approval.
