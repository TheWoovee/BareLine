# Localization readiness

Bareline ships in English only (ADR-28). This page describes the string-resource
layer that lets a language pack replace the user interface text later (BIZ-30),
what already goes through it, and what does not yet. Nothing is translated.

## Resource table

`crates/settings/locales/en.toml` is the English resource table. It is compiled
into the binary (`LocalePack::english`) and maps stable keys to text:

| Key | Text |
|---|---|
| `command.<command id>` | The title the command registers, for example `"command.file.save_as" = "Save As…"`. |
| `menu.<English caption>` | A submenu caption, for example `"menu.Recent Files" = "Recent Files"`. |
| `settings.*` | Settings page labels and messages. |
| `setting.<key>.title`, `setting.<key>` | Setting titles and descriptions, keyed from the setting definitions. |

Values may name parameters as `{name}`; a literal brace is written `{{` or `}}`.

A language pack is a file with the same shape, `version = 1`, `locale`,
`direction` (`ltr` or `rtl`) and a `[messages]` table, installed as
`locales/<locale>.toml` next to the user settings file. Every message a pack
leaves out falls back to English, and a pack whose message uses different
parameters from the English one is rejected.

Command titles are still written where each command registers, and that
literal stays the English fallback. The resource must hold exactly the same
text; the checks below compare them.

**Exempt:** the codec commands `encoding.interpret.*` and `encoding.convert.*`
are titled with the encoding's standard name (`UTF-8`, `Windows-1252`, …), which
is not translated. Labels computed at run time from data, such as a recent file
name or an open document's name, are data rather than resources.

## What reads the resources today

- The menu bar: every command item and every submenu caption
  (`sync_commands_localized` in `crates/platform-windows/src/native.rs`, fed by
  the shell with `command.<id>` and `menu.<caption>` keys). A command whose
  state carries a run-time label (Window list entries, Recent files, saved
  macros, encoding status items) shows that label unchanged; only the
  registered title goes through `command.<id>`.
- Context menus built from commands (tab and panel menus).
- The settings page labels.

## Locale selection

The `language.locale` setting (hidden in the settings page until a second locale
ships) picks the pack:

- `system`, the default, follows the Windows display language
  (`GetUserDefaultUILanguage`, named by `LCIDToLocaleName`, in
  `bareline_platform_windows::system_ui_language`). An English display
  language, or one with no installed pack, uses the built-in English quietly.
- Any other value, such as `de-DE`, loads `locales/de-DE.toml`. If that pack is
  missing or invalid, the settings page reports it and the current language stays in use.

`bareline_settings::requested_locale` holds this rule and is unit tested.

## Adding a command or a submenu

When you register a command, add its line to `en.toml` in the `command.` block,
keeping the block sorted:

```toml
"command.view.zoom.in" = "Zoom In"
```

A new submenu caption in `crates/app/src/menus.rs` needs a `menu.` line in the
same way. If you rename a command, change its resource too.

Three tests fail, and print the exact lines to add, when a registered command
or a menu caption has no matching resource:

| Test | Scope |
|---|---|
| `inventory::route_tests::every_command_and_menu_has_an_english_resource` (`apps/bareline`) | The production registry and its menus. This is the complete check. |
| `menus::tests::menu_captions_and_crate_commands_have_english_resources` (`bareline-app`) | Every caption in the menu tree and the commands registered by `bareline-app`. |
| `command_titles_and_menu_captions_are_keyed_english_resources` (`bareline-settings`) | The built-in shell commands and the top-level menus. |

`python scripts/check_localization.py` runs the same comparison without
compiling, by reading the registration tables and the menu tree; its
`--missing` option prints the lines to add. It reads source text, so the Rust
tests remain the authority.

**Parallel branches.** A branch that registers commands passes these tests only
once it adds their resource lines, so commands added on other branches are
accepted after a merge exactly when they carry their keys. Resource lines are
independent sorted lines, so merge conflicts in `en.toml` resolve by keeping
both sides.

## Remaining string sites

These user-visible literals are still written in code rather than read from
resources: status and notice texts, dialog and panel labels, toolbar tooltips,
the command palette (which shows and searches `CommandSpec::title` directly),
accessibility names and error messages. Counts come from
`python scripts/check_localization.py --inventory`: string literals that start
with a capital letter or digit and contain a space or end in punctuation, minus
text already served from resources. The heuristic also counts some diagnostics
and log text, so treat the numbers as the size of the work, not an exact list.

Counted on 2026-10-01:

| Crate | Literal sites |
|---|---:|
| `apps/bareline` | 1079 |
| `crates/app` | 569 |
| `crates/editor-surface` | 421 |
| `crates/file-io` | 161 |
| `crates/syntax` | 99 |
| `crates/platform-windows` | 79 |
| `crates/macros` | 75 |
| `crates/settings` | 65 |
| `crates/search` | 40 |
| `crates/commands` | 26 |
| `crates/platform` | 24 |
| `crates/distribution` | 21 |
| `crates/ui` | 4 |
| `apps/extension-host` | 3 |
| `apps/update-helper` | 1 |
| **Total** | **2667** |

| Module (20 or more sites) | Literal sites |
|---|---:|
| `app::workspace` | 134 |
| `editor_surface::paged_view` | 102 |
| `bareline::windows_app::extensions` | 90 |
| `file_io::paged_recovery` | 75 |
| `bareline::windows_app::search::replace` | 72 |
| `editor_surface::paged_power` | 68 |
| `bareline::windows_app::compare` | 65 |
| `bareline::windows_app` | 62 |
| `bareline::windows_app::recovery` | 61 |
| `app::macros` | 58 |
| `bareline::windows_app::macros` | 55 |
| `app::find` | 53 |
| `bareline::windows_app::utilities` | 51 |
| `bareline::windows_app::launch` | 50 |
| `syntax::outline` | 50 |
| `app::settings` | 49 |
| `syntax::udl` | 46 |
| `bareline::windows_app::power_stream` | 43 |
| `editor_surface` | 43 |
| `macros::process` | 43 |
| `app::language` | 40 |
| `app::workspace::encoding` | 40 |
| `bareline::windows_app::watch` | 39 |
| `settings::model` | 38 |
| `editor_surface::paged_transfer` | 37 |
| `bareline::windows_app::extensions::ui` | 35 |
| `bareline::windows_app::lifecycle` | 35 |
| `bareline::windows_app::shell_integration` | 35 |
| `bareline::windows_app::views` | 35 |
| `editor_surface::power::captured` | 33 |
| `bareline::windows_app::accessibility` | 32 |
| `macros` | 32 |
| `bareline::windows_app::power` | 31 |
| `search::replace_disk` | 29 |
| `file_io::paged_group_recovery` | 28 |
| `bareline::windows_app::update` | 27 |
| `bareline::windows_app::macros::location` | 25 |
| `editor_surface::paged_navigation` | 25 |
| `editor_surface::power::consumer` | 24 |
| `bareline::windows_app::workspace_panels` | 22 |
| `app::search_panel` | 21 |
| `distribution::importer` | 21 |
| `app::encoding` | 20 |
| `bareline::windows_app::search::folder` | 20 |
| Other modules | 673 |

The next migration steps, in order of reach: the command palette (show and
search the localized title), status and notice texts, then dialog and panel
labels. Community language packs come before any non-English marketing
(P4-07).
