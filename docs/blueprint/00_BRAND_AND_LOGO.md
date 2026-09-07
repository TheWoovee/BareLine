# Bareline — Product Name, Brand and Logo Direction

**Version:** 1.2 (2026-09-05)  
**Selected name:** Bareline  
**Tagline:** *Plain text. Full power. No weight.*  
**Product type:** ultra-light, native-feeling, general-purpose text/code editor  
**Initial platform:** Windows 10/11 x64, with architecture kept portable to Linux and macOS  
**Licensing of the brand:** the name, wordmark, logo and app icon are trademarks of the product owner and are **not** covered by the MPL-2.0 code license. Forks must rename. See `06_OPEN_SOURCE_AND_DISTRIBUTION.md`.

## Why this name

“Bareline” communicates the product promise without tying the product to Notepad++ branding: the user works directly with lines of text, without IDE weight, visual clutter, mandatory cloud services, or an embedded browser runtime. It is short enough for an executable (`bareline.exe`), command (`bareline`), file associations, menus and iconography.

A quick web scan on 2026-09-05 found no prominent general-purpose text editor using **Bareline**. This is **not** legal trademark clearance; do a formal trademark/domain review before the first public release.

## Brand promise

1. **Instant:** opening the app or a file feels immediate. Measured, not claimed.
2. **Trustworthy:** a crash, update or plugin must never silently destroy text.
3. **Plain-first:** text editing is the center; secondary panels appear only when asked for.
4. **Power on demand:** regex, multiple cursors, large files, syntax, workspaces, macros and extensions remain one shortcut away.
5. **No forced cloud:** everything works offline. No account, no telemetry.
6. **Open:** the code is public, the build is reproducible, the release is signed.

## Design tokens (authoritative)

These values are used by the theme files in `ui-assets/themes/`, by the compare and syntax token defaults, and verbatim inside every Imagen prompt in `08_IMAGEN_PROMPTS.md`. Change them here first.

### Accent (cyan-teal)

| Token | Light | Dark | Use |
|---|---|---|---|
| `accent` | `#14A89A` | `#2ED3C4` | decorative accent; use focus.ring and accent.text for functional contrast |
| `accent.text` | `#0B7A6F` | `#2ED3C4` | accent used as text (passes 4.5:1 on its surface) |
| `selection` | `#14A89A` at 20% | `#2ED3C4` at 22% | editor selection, list selection |
| `caret` | `#0B7A6F` | `#2ED3C4` | primary caret; secondary carets retain opaque outlines |

### Surfaces and text

| Token | Light (warm white) | Dark (charcoal) |
|---|---|---|
| `surface.editor` | `#FAF8F5` | `#1F2328` |
| `surface.chrome` | `#F1EEE9` | `#181B1F` |
| `surface.elevated` | `#FFFFFF` | `#262B31` |
| `surface.currentLine` | `#F3F0EB` | `#262B31` |
| `border` | `#DDD8D0` | `#343A42` |
| `text` | `#23272B` | `#E6E8EA` |
| `text.muted` | `#5C6570` | `#9AA3AD` |
| `text.gutter` | `#5C6570` | `#9AA3AD` |
| `danger` | `#C4463D` | `#F0705F` |
| `warning` | `#B7791F` | `#E3B341` |
| `success` | `#2E8B57` | `#5FCF80` |

### Compare (semantic diff tokens)

| Token | Light background | Dark background | Gutter glyph |
|---|---|---|---|
| `diff.added` | `#E3F5EA` | `#1E3A2F` | `+` |
| `diff.removed` | `#FBE4E4` | `#3D2426` | `−` |
| `diff.changed` | `#FFF3D6` | `#3A3420` | `~` |
| `diff.moved` | `#E6EEF8` | `#23303F` | `↕` |
| `diff.current` | `focus.ring` outline, 2 px | `focus.ring` outline, 2 px | filled marker |

Intraline emphasis uses the same hue at roughly double saturation. A color-blind-safe alternate palette (blue/orange) ships as `compare-cb.toml`. State is never communicated by color alone.

### Syntax (default editor theme)

| Token | Light | Dark |
|---|---|---|
| `syntax.keyword` | `#7B3FB5` | `#C79BFF` |
| `syntax.string` | `#2E7D32` | `#A5D6A7` |
| `syntax.number` | `#B45309` | `#F5B76B` |
| `syntax.comment` | `#5C6570` | `#9AA3AD` |
| `syntax.function` | `#0B63C5` | `#8AB4F8` |
| `syntax.type` | `#0B7A6F` | `#2ED3C4` |
| `syntax.operator` | `#23272B` | `#E6E8EA` |

### Typography and metrics

- Chrome font: system UI font (Segoe UI Variable on Windows). No proprietary font files ship.
- Editor font: user-selectable monospace; default resolves Cascadia Mono, then Consolas, then the system monospace.
- Base chrome size 13 px at 100%; editor default 12 pt.
- Corner radius: 6 px for panels, popovers and buttons; 4 px for chips; the editor canvas is square.
- Borders before shadows. One 1 px border in `border`; shadows only on popovers at 8% black.
- Density: row height 28 px in lists and trees at 100%; tabs 34 px; status bar 24 px.

## Logo concept

Use a single continuous geometric monoline stroke that simultaneously suggests:

- a **lowercase `b`** for Bareline;
- a **folded document edge** at the top-right of the bowl;
- two horizontal **text lines** inside the bowl;
- a subtle forward **notch** cut into the lower-right of the bowl implying speed.

Construction grid: 24 × 24 units. Stem on the left occupies the full height; bowl occupies the lower 14 units; stroke weight 3 units; corner radius 1.5 units; notch is a 45° cut of 2 units. Mark color is `text` on light, `text` on dark, with the notch or fold optionally in `accent`. Wordmark set in the system UI font, medium weight, letter-spaced 1%.

The icon must remain recognizable at 16, 20, 24, 32, 40, 48, 64, 128 and 256 px (the extra sizes cover 125% and 250% Windows scaling). Do not use a chameleon, leaf, pencil, floppy disk, quill or a Windows Notepad page icon.

### App icon (ICO) rules

- Background: rounded square, radius 22% of side, filled `surface.chrome` dark (`#181B1F`) for the primary icon; a light variant fills `#F1EEE9`.
- Mark: `#E6E8EA` on the dark tile, `#23272B` on the light tile; notch in `accent`.
- At 16 and 20 px drop the notch and the fold; keep the `b` only.
- Deliver PNG masters at 1024 px, SVG source, and `.ico` with all nine sizes.

## Adopted vector deliverable

Adopt the two interior text-line variant shown in bareline-app-icon.png and bareline-wordmark.png. Use [bareline-mark.svg](mockups/logo-brand/bareline-mark.svg), its [dark](mockups/logo-brand/bareline-mark-dark.svg) and [light](mockups/logo-brand/bareline-mark-light.svg) variants, and the [small-size source](mockups/logo-brand/bareline-mark-16.svg). Remove fold, notch and interior lines at 16 and 20 px. Keep the small source on the 24-unit construction grid.

Run `python -m pip install resvg-py==0.5.0 Pillow==12.3.0`, then `python scripts/render_icons.py`. resvg-py rasterizes SVG; Pillow assembles the nine-size ICO. These are asset tooling dependencies, not application runtime dependencies. Treat the ICO as a draft until a human views the native 16 px output and records approval in mockups/NOTES.md.

## Imagen briefs

All prompts for the logo, the app icon, the wordmark and every UI screen live in `08_IMAGEN_PROMPTS.md` and repeat the tokens above verbatim. Generated images are directional. The production logo must be redrawn as vector from the chosen concept; the production UI follows `01_REQUIREMENTS.md`, not the pixels.

## v1.2 accessible functional tokens

| Token | Light | Dark | Contract |
|---|---|---|---|
| focus.ring | #0B7A6F | #2ED3C4 | 2 px outline, at least 3:1 against adjacent surface |
| border.interactive | #5C6570 | #9AA3AD | Important control boundaries; decorative borders need not convey state |
| caret | #0B7A6F | #2ED3C4 | Functional contrast; secondary carets retain an opaque outline |
| mark.style1 | #E3F5EA | #1E3A2F | 1 / underline |
| mark.style2 | #FFF3D6 | #3A3420 | 2 / dotted underline |
| mark.style3 | #E6EEF8 | #23303F | 3 / double underline |
| mark.style4 | #FBE4E4 | #3D2426 | 4 / dashed underline |
| mark.style5 | #EEE5F6 | #342740 | 5 / side marker |

Use normal text over mark/diff backgrounds and measure composite contrast. The light decorative accent alone does not qualify as a focus outline. [Machine-readable tokens](mockups/design-tokens.json) mirror these revised values. Validate normal text at 4.5:1 and essential non-text indicators at 3:1. Raster pixel colors and font rendering still require implementation QA.

The existing PNG logo concepts are retained. Final vector masters, optical small-size adjustment and multi-size ICO remain production work; this revision does not pretend that a raster is an editable vector. The hero remains an archived direction where it conflicts with Windows chrome.
