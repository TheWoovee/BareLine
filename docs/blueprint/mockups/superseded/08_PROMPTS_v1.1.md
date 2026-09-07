# Bareline Imagen Prompts: Logo, Icon, Wordmark and UI Screens

**Version:** 1.2 (2026-09-05)  
**Source of truth for content:** `01_REQUIREMENTS.md` section 10 (screen list) and `00_BRAND_AND_LOGO.md` (tokens).  
**Decision:** ADR-29, ADR-30 in `07_DECISION_LOG.md`.

## 1. How to get from a 5 to a 10

The current boards scored 5/10 and 7/10 because they are collages that contradict the requirements. The fixes are procedural, not artistic:

1. **One screen per image.** Never ask for a board. Boards force tiny text and invite invented features.
2. **Paste the style prefix verbatim** before every UI prompt. It carries the exact hex tokens, so every screen shares one palette.
3. **Name every on-screen string.** Imagen renders short text well and invents long text badly. Each prompt lists the exact words; anything not listed should not appear.
4. **Say what must not appear, in positive form.** Imagen 3 and 4 have no negative prompt, so the prefix says "no toolbar, no sidebar" as part of the description.
5. **Settings.** Model: Imagen 4 (or the newest Imagen). Aspect ratio 16:9 for screens, 1:1 for the mark and icon, 16:9 for the wordmark. Four samples per prompt, pick one, upscale 2×. If the API exposes a seed (Vertex does when the watermark is off), reuse one seed for A1, C1, H and I so the base editor looks identical across them.
6. **Reject on sight** any output that shows: a toolbar row of icons, a sidebar or minimap where the prompt did not ask for one, the words "Large File Mode" or "Read-only", the word "Plugins", a terminal, a source-control panel, a leaf or chameleon logo, a green accent, a floating search dialog, or line numbers that skip or repeat.
7. **Finish in Figma.** Treat the generated image as a base layer. Replace the status bar text, tab labels and menu labels with real text layers using the exact strings. This is where the last two points come from.
8. **Pairs.** Generate A and F in both dark and light. Everything else in dark only for v1.
9. **Logo is a concept, not a file.** Pick the best mark, redraw it as vector on the 24-unit grid in `00_BRAND_AND_LOGO.md`, export SVG and a nine-size ICO.
10. **File names.** Save finals under `mockups/` with the names in `mockups/README.md` and note the prompt ID used.
11. **Lessons from the first batch (2026-09-05).** Screens generated without an explicit menu-bar sentence dropped the menu bar, and four screens repeated a line number. The prefix now forces the menu bar and status bar; repeated line numbers are fixed in Figma. Keep sample text free of out-of-scope words: no `[lsp]`, `auto_save`, `plugins`, `telemetry`, `tokio` or `language server` in code or log samples.

## 2. Shared style prefix (paste before every UI prompt)

### Dark prefix

```text
High-fidelity UI screenshot of a Windows 11 desktop text editor named Bareline, rendered as a crisp flat vector mockup. One 1440 by 900 application window fills the frame edge to edge; no desktop wallpaper, no device frame, no hands, no people, no marketing text outside the window. Visual language: calm, minimal, native Windows 11, thin 1 px borders, 6 px corner radii on panels, no drop shadows except on popovers, compact density with generous spacing. Dark theme colors: editor background #1F2328, window chrome #181B1F, elevated panels #262B31, borders #343A42, text #E6E8EA, muted text #9AA3AD, gutter line numbers #6B7480, accent cyan-teal #2ED3C4 used only for focus rings, the active tab underline, the caret and selection. Typography: Segoe UI 13 px for chrome, Cascadia Mono 12 pt for code. Syntax colors: keywords #C79BFF, strings #A5D6A7, numbers #F5B76B, comments #7F8A96, functions #8AB4F8, types #2ED3C4. Title bar shows a tiny monoline lowercase b logo and the word Bareline; no leaf, no chameleon. Directly under the title bar, on every screen, a native menu bar with the words File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help. Below the editor area, on every screen, a one-line status bar. No toolbar row of icons anywhere. All text is sharp and legible; render only the strings specified below and invent no other labels.
```

### Light prefix

```text
High-fidelity UI screenshot of a Windows 11 desktop text editor named Bareline, rendered as a crisp flat vector mockup. One 1440 by 900 application window fills the frame edge to edge; no desktop wallpaper, no device frame, no hands, no people, no marketing text outside the window. Visual language: calm, minimal, native Windows 11, thin 1 px borders, 6 px corner radii on panels, no drop shadows except on popovers, compact density with generous spacing. Light theme colors: editor background warm white #FAF8F5, window chrome #F1EEE9, elevated panels #FFFFFF, borders #DDD8D0, text #23272B, muted text #5C6570, gutter line numbers #9A968F, accent cyan-teal #14A89A used only for focus rings, the active tab underline, the caret and selection. Typography: Segoe UI 13 px for chrome, Cascadia Mono 12 pt for code. Syntax colors: keywords #7B3FB5, strings #2E7D32, numbers #B45309, comments #7A8290, functions #0B63C5, types #0B7A6F. Title bar shows a tiny monoline lowercase b logo and the word Bareline; no leaf, no chameleon. Directly under the title bar, on every screen, a native menu bar with the words File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help. Below the editor area, on every screen, a one-line status bar. No toolbar row of icons anywhere. All text is sharp and legible; render only the strings specified below and invent no other labels.
```

## 3. UI screen prompts (16:9)

### A1 Main editor, dark (`bareline-main-editor-dark.png`)

```text
[DARK PREFIX] Layout from top: native menu bar with the words File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help. Directly below, a tab strip with exactly six tabs: "main.rs" active with a cyan-teal underline, "config.toml" with a small pin icon instead of a close button, "notes.md", "server.log", "query.sql", and "Untitled 1" with a small dot meaning unsaved. No sidebar on either side, no minimap, no bottom panel. The editor fills at least 80 percent of the window: line numbers 1 to 24 in a narrow gutter, about twenty lines of Rust code with subtle syntax color, the current line softly highlighted #262B31, a visible cyan-teal caret, and a thin 8 px scrollbar at the right edge. One-line status bar at the bottom reading left to right: "Rust", "2.4 KB · 24 lines", "Ln 12, Col 8", "LF", "UTF-8", "INS". A faint hint at the bottom right corner of the editor: "Ctrl+Shift+P for commands". Nothing else in the window.
```

### A2 Main editor, light (`bareline-main-editor-light.png`)

```text
[LIGHT PREFIX] Same layout as the dark main editor: native menu bar with File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help; six tabs "main.rs" active with a cyan-teal underline, "config.toml" pinned with a pin icon, "notes.md", "server.log", "query.sql", "Untitled 1" with an unsaved dot; no sidebar, no minimap, no bottom panel; editor at least 80 percent of the window with line numbers 1 to 24, about twenty lines of Rust code with subtle syntax color, current line highlighted #F3F0EB, cyan-teal caret, thin scrollbar. Status bar: "Rust", "2.4 KB · 24 lines", "Ln 12, Col 8", "LF", "UTF-8", "INS". Faint hint "Ctrl+Shift+P for commands" at the bottom right of the editor.
```

### B Workspace and split editor (`bareline-workspace-split.png`)

```text
[DARK PREFIX] Native menu bar as usual. A left sidebar 220 px wide titled "Workspace" with a compact folder tree: "bareline", "crates", "document", "lib.rs", "piece_tree.rs", "search", "Cargo.toml", "README.md", using thin chevrons and small file icons. The center is split into two equal editor panes side by side separated by a 1 px divider: the left pane has one tab "settings.toml" and shows TOML configuration with syntax color; the right pane has one tab "app.log" and shows log lines with timestamps and levels. At the top of the divider a small discreet linked-scroll icon in cyan-teal indicating synchronized scrolling. A right panel 240 px wide titled "Outline" listing "struct PieceTree", "fn insert", "fn delete", "fn split_at", "impl Document" with small symbol glyphs. No bottom panel. Status bar: "TOML", "8.1 KB · 212 lines", "Ln 40, Col 1", "LF", "UTF-8", "INS".
```

### C1 Inline find bar (`bareline-find-bar.png`)

```text
[DARK PREFIX] The main editor with the same six tabs and Rust code. Docked directly below the tab strip and above the code, a compact find bar 34 px tall: a text field containing "TODO" with a cyan-teal focus ring, three small toggle buttons labeled "Aa", "ab" and ".*" where ".*" is active in cyan-teal, a match counter "3 of 41", up and down arrow buttons, a chevron labeled "Replace", and a close x. In the code, every "TODO" is highlighted with a soft translucent cyan-teal background and the current match has a 1 px cyan-teal outline. No dialog, no separate window, no bottom panel. Status bar as in the main editor.
```

### C2 Search panel with grouped results (`bareline-search-panel.png`)

```text
[DARK PREFIX] The editor occupies the top 60 percent of the window with the usual tabs and Rust code. A bottom panel occupies the lower 40 percent with three tabs "Find", "Replace", "Files" where "Files" is active with a cyan-teal underline. One compact row of controls: a query field "TODO", toggles "Aa", "ab", ".*", a folder field "D:\src\bareline", a filter field "*.rs, *.toml", and a cyan-teal "Search" button. Below, results grouped by file with expand chevrons: "crates/document/src/piece_tree.rs  5" expanded to show three indented result lines with line numbers 41, 88, 130 and code excerpts in which TODO is bold; then "crates/search/src/lib.rs  3" collapsed; then "README.md  1" collapsed. A muted status line inside the panel: "41 matches in 9 files · 0.3 s" with a small "Cancel" link. The panel is docked, not floating. Status bar at the very bottom as usual.
```

### D1 Settings (`bareline-settings.png`)

```text
[DARK PREFIX] The editor area is replaced by a Settings view inside the same window. A left column 200 px wide lists categories: "Editor", "Appearance", "Files", "Search", "Keyboard", "Languages", "Extensions", "Advanced"; "Editor" is selected with a cyan-teal marker. At the top of the content area a search field with placeholder "Search settings". Content rows, each with a bold title, a one-line muted description, a control on the right, and a tiny monospace key in muted text beneath the title: "Font family" with dropdown "Cascadia Mono" and key "editor.font.family"; "Font size" with number field "12" and key "editor.font.size"; "Word wrap" with a toggle on and key "editor.wrap.mode"; "Show whitespace" with a toggle off and key "editor.render.whitespace"; "Tab width" with number field "4" and key "editor.tab.width"; "Highlight current line" with a toggle on and key "editor.currentLine.highlight". At the bottom a collapsed section header "Advanced (12)" with a chevron. Status bar visible.
```

### D2 Extensions manager (`bareline-extensions.png`)

```text
[DARK PREFIX] The editor area is replaced by an Extensions view inside the same window. Header "Extensions" with four tabs "Installed", "Discover", "Updates", "Disabled"; "Installed" active with a cyan-teal underline. Under the tabs a slim info banner with a shield icon reading "Extensions run isolated in a separate process. Bareline never loads extension code." Three cards stacked vertically, each with an icon, a name and version, a one-line description, small rounded permission chips, and a toggle on the right: "JSON Tools 1.0.0", "Format, minify, validate, tree view", chips "document.read", "document.edit", "ui.panel", toggle on; "XML Tools 1.0.0", "Format, validate, XPath", chips "document.read", "document.edit", toggle on; "Hex View 1.0.0", "Read-only bytes and text view", chip "document.read", toggle on. A muted footer line: "Runtime: bareline-exthost 1.0.0 installed · 18 MB". Status bar visible.
```

### E Huge log, editing normally (`bareline-huge-log.png`)

```text
[DARK PREFIX] Tab strip with one active tab "access-2026-09.log". The editor shows dense web server log lines with timestamps, IP addresses, request paths and status codes in monospace with light syntax color on numbers and on the words INFO, WARN, ERROR. The gutter shows line numbers in the hundreds of millions such as "241,558,204" through "241,558,236". The scrollbar thumb is small and near the top. A compact find bar below the tab strip contains the query "ERROR 5" and a counter "1,204 so far…" with a small "Stop" link, indicating results are streaming in. A visible cyan-teal caret and a normal text selection show the file is fully editable. Status bar: "Log", "3.8 GB · indexing in background", "Ln 241,558,204, Col 1", "LF", "UTF-8", "INS", and a thin cyan-teal progress line along the top edge of the status bar filled to about 40 percent. There is no banner, no read-only label, and no mention of a large file mode.
```

### F1 Compare workspace, dark (`bareline-compare-dark.png`)

```text
[DARK PREFIX] Tab strip with one active tab "Compare: config_v1.toml ↔ config_v2.toml". Below it a compact compare toolbar: left source label "D:\cfg\config_v1.toml", a swap icon, right source label "D:\cfg\config_v2.toml", a dropdown "Ignore whitespace", a button "Options", previous and next arrow buttons, a counter "4 / 27", a button "Recompare", and "Close". Two equal editor panes side by side, each with line numbers and a narrow diff gutter. Left pane: two removed lines with background #3D2426 and a "−" glyph in the gutter. Right pane: three added lines with background #1E3A2F and a "+" glyph. Both panes: one changed line pair with background #3A3420, a "~" glyph, and the changed words emphasized in a stronger amber; one moved block with background #23303F and a "↕" glyph; the current difference outlined 2 px in cyan-teal on both sides, kept aligned with an empty spacer band in the other pane. A thin overview strip at the far right shows colored markers for each hunk and a viewport indicator. Status bar: "Compare", "Added 9 · Removed 6 · Changed 11 · Moved 1", "Ln 42, Col 1", "UTF-8".
```

### F2 Compare options popover, light (`bareline-compare-options.png`)

```text
[LIGHT PREFIX] The compare workspace in light theme is visible behind a slightly dimmed layer. A centered popover 520 px wide titled "Compare options" with two tabs "General" and "Colors"; "Colors" active. A segmented control "Light | Dark | System" with "Light" selected. Five rows, each with a state name, a background swatch, an accent swatch, a gutter glyph preview and a small reset icon: "Added" swatch #E3F5EA glyph "+"; "Removed" swatch #FBE4E4 glyph "−"; "Changed" swatch #FFF3D6 glyph "~"; "Moved" swatch #E6EEF8 glyph "↕"; "Current" a cyan-teal outline swatch with a filled marker glyph. A toggle labeled "Color-blind palette (blue / orange)". Beneath, five checkboxes: "Ignore leading/trailing whitespace", "Ignore all whitespace", "Ignore blank lines", "Ignore case", "Ignore EOL style". Buttons at the bottom right: "Use theme defaults" and a cyan-teal "Done".
```

### G Recovery Center (`bareline-recovery-center.png`)

```text
[DARK PREFIX] The main editor is visible behind a dimmed layer. A centered panel 720 px wide titled "Recovery Center" with the subtitle "Bareline closed unexpectedly. These documents have unsaved changes." Three rows, each with a document icon, a name, a path or note, an age and a size: "notes.md", "D:\docs\notes.md", "edited 2 min ago", "+14 −3 lines"; "Untitled 2", "never saved", "1.2 KB"; "schema.sql", "D:\work\schema.sql", "edited 41 s ago", "+2 lines". The first row is expanded to show a small side-by-side diff preview with added lines on #1E3A2F and removed lines on #3D2426. Each row has four buttons: "Recover", "Save As…", "Discard", "Keep for later"; "Recover" is cyan-teal. A muted footer line: "Recovery data is stored locally with checksums and never replaces your files."
```

### H Command palette (`bareline-command-palette.png`)

```text
[DARK PREFIX] The main editor with six tabs and Rust code, slightly dimmed. A centered palette 640 px wide positioned near the top of the window: a text field containing "sort" with a cyan-teal focus ring. A results list of five rows, each with a command title on the left, a muted menu path beneath it, and a shortcut chip on the right: "Sort Lines Ascending", "Edit › Line Operations", chip "Ctrl+Alt+S"; "Sort Lines Descending", "Edit › Line Operations"; "Sort Lines as Integers", "Edit › Line Operations"; "Remove Duplicate Lines", "Edit › Line Operations"; "Uppercase", "Edit › Convert Case", chip "Ctrl+Shift+U". The first row is highlighted with a cyan-teal left marker. A footer hint: "↑↓ navigate · Enter run · Esc close".
```

### I Tail mode (`bareline-tail-mode.png`)

```text
[DARK PREFIX] Tab strip with one active tab "worker.log" that shows a small lock icon before the name. Under the tab strip a slim banner with a 1 px cyan-teal border reading "Following worker.log · Paused (scrolled up) · Resume ↓ · Unlock to edit". The editor shows log lines with timestamps and levels; the last visible lines are slightly emphasized. Status bar: "Log", "412 MB · +2.1 MB/s", "Ln 3,204,118, Col 1", "LF", "UTF-8", "Following".
```

## 4. Logo, icon and wordmark prompts

### L1 Logo mark, light background (1:1) (`bareline-logo-mark-light.png`)

```text
Minimal flat vector logo mark, centered and large on a plain warm white #FAF8F5 background, square composition. A single continuous monoline stroke forms a lowercase letter b: a tall vertical stem on the left and a bowl on the lower right made of two horizontal text lines that close at the right side. The top right corner of the bowl is folded over like the corner of a document page. A small 45 degree notch is cut into the lower right of the bowl to suggest forward speed. Stroke weight is one eighth of the glyph height with rounded caps and joins, geometric and precise as if drawn on a 24 unit grid. Mark color near black #23272B; only the notch is cyan-teal #14A89A. No words, no other letters, no leaf, no chameleon, no pencil, no quill, no gradient, no 3D, no shadow, no texture. Flat, timeless, technical, calm.
```

### L1b Logo mark, dark background (1:1) (`bareline-logo-mark-dark.png`)

```text
Minimal flat vector logo mark, centered and large on a plain charcoal #181B1F background, square composition. A single continuous monoline stroke forms a lowercase letter b: a tall vertical stem on the left and a bowl on the lower right made of two horizontal text lines that close at the right side. The top right corner of the bowl is folded over like the corner of a document page. A small 45 degree notch is cut into the lower right of the bowl to suggest forward speed. Stroke weight is one eighth of the glyph height with rounded caps and joins, geometric and precise. Mark color light gray #E6E8EA; only the notch is cyan-teal #2ED3C4. No words, no other letters, no leaf, no chameleon, no pencil, no quill, no gradient, no 3D, no shadow, no texture. Flat, timeless, technical, calm.
```

### L1c Exploration sheet (1:1, run once to choose a direction)

```text
A clean exploration sheet of nine flat vector logo marks arranged in a 3 by 3 grid on a warm white #FAF8F5 background with generous spacing. Every mark is a monoline lowercase letter b built from a left vertical stem and a bowl made of two horizontal text lines, with a folded document corner at the top right of the bowl and a small notch at the lower right. The nine versions vary only in stroke weight from thin to bold, corner radius from sharp to round, and notch size from subtle to pronounced. All marks near black #23272B with the notch in cyan-teal #14A89A. No words, no numbering, no other symbols, no leaf, no chameleon.
```

### L2 App icon tiles (1:1) (`bareline-app-icon.png`)

```text
Windows 11 style flat app icon for a text editor, presented as three tiles in a row on a neutral #F1EEE9 background: large, medium and small. Each tile is a rounded square with corner radius 22 percent filled charcoal #181B1F with a subtle 1 px inner border #343A42. Inside, centered and occupying 60 percent of the tile, a monoline lowercase letter b formed by a left vertical stem and a bowl of two horizontal text lines, with a folded top right corner and a small notch at the lower right; the mark is light gray #E6E8EA and the notch is cyan-teal #2ED3C4. The smallest tile shows a simplified version of the same b without the fold and without the notch. No text, no gloss, no gradient, no shadow, flat vector.
```

### L2b App icon, light tile variant (1:1) (`bareline-app-icon-light.png`)

```text
Windows 11 style flat app icon for a text editor, one large rounded square tile with corner radius 22 percent filled warm light #F1EEE9 with a subtle 1 px inner border #DDD8D0, centered on a charcoal #181B1F background. Inside, occupying 60 percent of the tile, a monoline lowercase letter b formed by a left vertical stem and a bowl of two horizontal text lines, with a folded top right corner and a small notch at the lower right; the mark is near black #23272B and the notch is cyan-teal #14A89A. No text, no gloss, no gradient, no shadow, flat vector.
```

### L3 Wordmark with tagline (16:9) (`bareline-wordmark.png`)

```text
Horizontal brand lockup on a plain warm white #FAF8F5 background, centered, generous whitespace. At the left, the monoline lowercase b mark: left vertical stem, bowl of two horizontal text lines, folded top right corner, small notch at the lower right, near black #23272B with the notch in cyan-teal #14A89A. To its right the word "Bareline" set in a clean geometric humanist sans serif similar to Segoe UI Variable, medium weight, slight 1 percent letter spacing, color #23272B. Beneath the word, smaller and in muted gray #5C6570, the tagline "Plain text. Full power. No weight." Flat vector, no other elements, no shadow, no gradient.
```

### L3b Wordmark, dark (16:9) (`bareline-wordmark-dark.png`)

```text
Horizontal brand lockup on a plain charcoal #181B1F background, centered, generous whitespace. At the left, the monoline lowercase b mark: left vertical stem, bowl of two horizontal text lines, folded top right corner, small notch at the lower right, light gray #E6E8EA with the notch in cyan-teal #2ED3C4. To its right the word "Bareline" in a clean geometric humanist sans serif similar to Segoe UI Variable, medium weight, color #E6E8EA. Beneath, smaller and in muted gray #9AA3AD, the tagline "Plain text. Full power. No weight." Flat vector, no other elements, no shadow, no gradient.
```

### L4 Release hero image, optional (16:9) (`bareline-hero.png`)

```text
Product hero image for an open-source Windows text editor. The window uses Windows 11 chrome with minimize, maximize and close buttons at the top right and a native menu bar reading File, Edit, Search, View, Encoding, Language, Settings, Macro, Run, Tools, Window, Help; no macOS traffic-light buttons. Background charcoal #181B1F with a single thin cyan-teal #2ED3C4 horizontal accent line near the top. Top left: a small brand lockup with a monoline lowercase b mark and the word "Bareline" in light gray. Center: one 1440 by 900 application window of the Bareline editor rendered as a crisp flat mockup, slightly scaled down and floating with a very soft shadow: native menu bar, six tabs with one pinned, an editor filling most of the window with Rust code in subtle syntax color, line numbers, a one-line status bar reading "Rust", "2.4 KB · 24 lines", "Ln 12, Col 8", "LF", "UTF-8", "INS". No sidebar, no toolbar, no minimap. Beneath the window, one line of white text: "Plain text. Full power. No weight." Nothing else: no feature grid, no icon row, no badges, no people.
```

## 5. Review checklist for every generated screen

Score each item 0 or 1; ship at 9 or 10, otherwise regenerate or fix in Figma.

1. Only the strings listed in the prompt appear; none are garbled.
2. Colors match the tokens; accent is cyan-teal, never green.
3. Logo is the monoline b; no leaf, chameleon, pencil or quill.
4. No toolbar row unless the prompt asked for one.
5. Sidebars, minimap and bottom panel appear only when the prompt asked.
6. Editor occupies at least 80 percent of the window in A1, A2, C1, E, H, I.
7. Status bar shows exactly six segments in the specified order.
8. Line numbers are sequential with no skips or repeats.
9. The feature shown matches the requirement (for example E is editable with no read-only label; C2 is a docked panel, not a dialog).
10. Nothing out of scope is visible: no terminal, no source control, no "Plugins", no "Auto Save".
