// SPDX-License-Identifier: MPL-2.0
//! The hand-authored menu taxonomy. Instead of emitting menu items in
//! registration order (which produced a File menu that opened with *Exit* and an
//! Edit menu taller than the screen), the twelve top-level menus are laid out
//! here as ordered trees with separators and submenus.
//!
//! Every shell command has an explicit home: a place in [`TREE`], a declared
//! taxonomy menu path (the codec lists under Encoding), or a palette-only or
//! internal presentation. [`MenuModel::curated`] still routes a command with no
//! home into Tools ▸ Other so nothing disappears, but the shell's tests fail if
//! anything lands there (UI-04). Command IDs listed here that are not registered
//! (optional features) are simply skipped.

use bareline_commands::MenuTemplate::{Command as C, Separator as Sep, Submenu as Sub};
use bareline_commands::{CommandRegistry, MenuModel, MenuPlacement, MenuTemplate};

/// Build the live menu model for `registry` from [`TREE`].
pub fn curated_model(registry: &CommandRegistry) -> MenuModel {
    MenuModel::curated(registry, TREE)
}

/// Recovery, retry, conflict and cancel plumbing: listed only while the state
/// it acts on exists, so the menus do not carry rows that almost never apply.
pub const WHEN_ENABLED: &[&str] = &[
    "file.transcode.resume",
    "file.transcode.cancel",
    "file.cancel_operations",
    "file.cancel_save_all",
    "file.save_conflict_compare",
    "file.save_conflict_save_elsewhere",
    "file.save_conflict_retain_other",
    "file.save_conflict_next",
    "file.retry_save_cleanup",
    "file.retry_save_recovery",
    "file.retry_open",
    "file.open_large_file_mode",
    "file.external.keep",
    "file.monitor.pause",
    "file.monitor.resume",
    "file.monitor.reopen",
    "file.monitor.unlock",
    "search.cancel",
    "search.cancel_panel",
    "search.close_panel",
    "search.close_find",
    "search.replacePreview.refresh",
    "search.replacePreview.apply",
    "search.replacePreview.toggleAll",
    "search.replacePreview.cancel",
    "search.replacePreview.rollback",
    "search.replacePreview.close",
    "compare.cancel",
    "migration.cancel",
    "macro.cancel",
    "macro.resume",
    "macro.reload",
    "encoding.failure",
    "encoding.binary",
];

/// Commands with no state that tells when they apply, or that act on a panel's
/// own selection: reachable from the palette (and panel context menus) only.
pub const PALETTE_ONLY: &[&str] = &[
    "profile.migration.retry",
    "recovery.retry",
    "recovery.save_as",
    "view.theme.cycle",
    "workspace.loadMore",
    "documents.save",
    "documents.close",
    "outline.cancelImport",
    "output.open_link",
    "utilities.cancel",
];

/// Apply [`WHEN_ENABLED`] and [`PALETTE_ONLY`] to the registered commands. Call
/// once every command is registered.
pub fn apply_menu_placement(registry: &mut CommandRegistry) {
    for (ids, placement) in [
        (WHEN_ENABLED, MenuPlacement::WhenEnabled),
        (PALETTE_ONLY, MenuPlacement::PaletteOnly),
    ] {
        for id in ids {
            if let Some(id) = registry.lookup(id) {
                let _ = registry.update_presentation(id, |meta| meta.menu = placement);
            }
        }
    }
}

pub const TREE: &[MenuTemplate] = &[
    Sub(
        "File",
        &[
            C("file.new"),
            C("file.open"),
            C("workspace.openFolder"),
            Sub(
                "Recent Files",
                &[
                    C("file.recent.0"),
                    C("file.recent.1"),
                    C("file.recent.2"),
                    C("file.recent.3"),
                    C("file.recent.4"),
                    C("file.recent.5"),
                    C("file.recent.6"),
                    C("file.recent.7"),
                    C("file.recent.8"),
                    C("file.recent.9"),
                    C("file.recent.10"),
                    C("file.recent.11"),
                    C("file.recent.12"),
                    C("file.recent.13"),
                    C("file.recent.14"),
                    Sep,
                    C("file.recent.clear"),
                ],
            ),
            Sep,
            C("file.save"),
            C("file.save_as"),
            C("file.save_all"),
            C("file.rename"),
            Sep,
            C("file.close"),
            Sub(
                "Close Multiple",
                &[
                    C("view.tabs.closeAll"),
                    C("view.tabs.closeOthers"),
                    Sep,
                    C("view.tabs.closeLeft"),
                    C("view.tabs.closeRight"),
                ],
            ),
            Sub(
                "Document",
                &[
                    C("file.copyPath"),
                    C("file.copyName"),
                    C("file.copyDirectory"),
                    Sep,
                    C("file.reveal"),
                    C("file.terminal"),
                    Sep,
                    C("file.save_copy"),
                    C("file.read_only"),
                    C("file.restore_closed"),
                    Sep,
                    C("file.external.check"),
                    C("file.external.reload"),
                    C("file.external.keep"),
                    C("file.external.auto_reload"),
                    Sub(
                        "Monitoring and Remote",
                        &[
                            C("file.monitor.start"),
                            C("file.monitor.pause"),
                            C("file.monitor.resume"),
                            C("file.monitor.reopen"),
                            C("file.monitor.unlock"),
                            Sep,
                            C("file.remote.open"),
                            C("file.remote.follow"),
                            C("file.remote.reload"),
                        ],
                    ),
                    Sep,
                    C("file.retry_open"),
                    C("file.open_large_file_mode"),
                    C("file.cancel_operations"),
                    C("file.cancel_save_all"),
                    C("file.transcode.resume"),
                    C("file.transcode.cancel"),
                    Sub(
                        "Save Conflict",
                        &[
                            C("file.save_conflict_compare"),
                            C("file.save_conflict_save_elsewhere"),
                            C("file.save_conflict_retain_other"),
                            C("file.save_conflict_next"),
                        ],
                    ),
                    Sub(
                        "Recovery",
                        &[
                            C("recovery.open"),
                            C("recovery.open_folder"),
                            C("recovery.restore_latest"),
                            C("recovery.retry"),
                            C("recovery.save_as"),
                            C("file.retry_save_cleanup"),
                            C("file.retry_save_recovery"),
                            C("profile.migration.retry"),
                        ],
                    ),
                ],
            ),
            Sep,
            C("utilities.print"),
            Sub(
                "Print Options",
                &[
                    C("utilities.printSelection"),
                    C("utilities.printNow"),
                    Sep,
                    C("utilities.printHeader"),
                    C("utilities.printFooter"),
                    C("utilities.printNumbers"),
                    C("utilities.printSyntax"),
                ],
            ),
            Sep,
            C("app.quit"),
        ],
    ),
    Sub(
        "Edit",
        &[
            C("edit.undo"),
            C("edit.redo"),
            Sep,
            C("edit.cut"),
            C("edit.copy"),
            C("edit.paste"),
            C("editor.paste.plainText"),
            C("editor.paste.fromHistory"),
            C("editor.clipboard.toggleHistory"),
            C("edit.delete"),
            C("edit.select_all"),
            Sep,
            Sub(
                "Line Operations",
                &[
                    C("editor.lines.duplicate"),
                    C("editor.lines.moveUp"),
                    C("editor.lines.moveDown"),
                    C("editor.lines.join"),
                    C("editor.lines.split"),
                    Sep,
                    C("editor.lines.removeEmpty"),
                    C("editor.lines.removeBlank"),
                    C("editor.lines.removeDuplicates"),
                    C("editor.lines.removeConsecutiveDuplicates"),
                    Sep,
                    C("editor.lines.hide"),
                    C("editor.lines.showAll"),
                ],
            ),
            Sub(
                "Case",
                &[
                    C("editor.case.upper"),
                    C("editor.case.lower"),
                    C("editor.case.title"),
                    C("editor.case.invert"),
                ],
            ),
            Sub(
                "Comment",
                &[C("editor.comment.toggleLine"), C("editor.comment.toggleBlock")],
            ),
            Sub(
                "Indent",
                &[
                    C("editor.indent"),
                    C("editor.unindent"),
                    Sep,
                    C("editor.tabs.toSpaces"),
                    C("editor.spaces.toTabs"),
                    Sep,
                    C("editor.whitespace.trim"),
                    C("editor.whitespace.trimStart"),
                    C("editor.whitespace.trimEnd"),
                ],
            ),
            Sub(
                "Bookmarks",
                &[
                    C("editor.bookmark.toggle"),
                    C("editor.bookmark.next"),
                    C("editor.bookmark.previous"),
                    C("editor.bookmark.clear"),
                    Sep,
                    C("editor.bookmark.selectLines"),
                    C("editor.bookmark.copyLines"),
                    C("editor.bookmark.cutLines"),
                    C("editor.bookmark.deleteLines"),
                ],
            ),
            Sub(
                "Multi-caret",
                &[
                    C("editor.selection.nextOccurrence"),
                    C("editor.selection.skipOccurrence"),
                    C("editor.selection.undoOccurrence"),
                    C("editor.selection.allOccurrences"),
                    C("editor.selection.expandLines"),
                    C("editor.selection.duplicate"),
                    C("editor.selection.rotatePrimary"),
                    C("editor.selection.escape"),
                    Sep,
                    C("editor.caret.above"),
                    C("editor.caret.below"),
                ],
            ),
            Sub(
                "Column",
                &[
                    C("editor.column.insert"),
                    C("editor.rectangle.paste"),
                    C("editor.rectangle.delete"),
                ],
            ),
            Sub(
                "Sort",
                &[
                    C("editor.lines.sortAscending"),
                    C("editor.lines.sortDescending"),
                    C("editor.lines.sortNumeric"),
                    C("editor.lines.sortIgnoreCase"),
                ],
            ),
        ],
    ),
    Sub(
        "Search",
        &[
            C("search.find"),
            C("search.find_next"),
            C("search.find_previous"),
            C("search.replace"),
            C("search.goto"),
            Sep,
            Sub(
                "Scope",
                &[
                    C("search.scope.current"),
                    C("search.scope.selection"),
                    C("search.open_documents"),
                    C("search.folder"),
                ],
            ),
            Sub(
                "Mode",
                &[
                    C("search.mode.literal"),
                    C("search.mode.extended"),
                    C("search.mode.regex"),
                    Sep,
                    C("search.mode"),
                ],
            ),
            Sub(
                "Options",
                &[
                    C("search.match_case"),
                    C("search.whole_word"),
                    C("search.dot_matches_newline"),
                ],
            ),
            Sub(
                "Current Document Replacement",
                &[C("search.replace_one"), C("search.replace_all"), C("search.close_find")],
            ),
            Sub(
                "Mark",
                &[
                    C("search.mark.style1"),
                    C("search.mark.style2"),
                    C("search.mark.style3"),
                    C("search.mark.style4"),
                    C("search.mark.style5"),
                    Sep,
                    C("search.mark.clearStyle1"),
                    C("search.mark.clearStyle2"),
                    C("search.mark.clearStyle3"),
                    C("search.mark.clearStyle4"),
                    C("search.mark.clearStyle5"),
                    C("search.mark.clearAll"),
                ],
            ),
            Sub(
                "Workspace Replacement",
                &[
                    C("search.replaceInFiles"),
                    C("search.replaceInWorkspace"),
                    C("search.replacePreview.refresh"),
                    C("search.replacePreview.apply"),
                    C("search.replacePreview.toggleAll"),
                    Sep,
                    C("search.replacePreview.preserveCase"),
                    C("search.replacePreview.includeBinary"),
                    C("search.replacePreview.backups"),
                    Sep,
                    C("search.replacePreview.cancel"),
                    C("search.replacePreview.rollback"),
                    C("search.replacePreview.close"),
                    Sep,
                    C("search.replaceBackups.manage"),
                    C("search.replaceBackups.delete"),
                    C("search.replaceBackups.prune"),
                ],
            ),
            Sub(
                "Active Search",
                &[C("search.cancel"), C("search.cancel_panel"), C("search.close_panel")],
            ),
        ],
    ),
    Sub(
        "View",
        &[
            C("view.toolbar_toggle"),
            C("view.toolbar_customize"),
            C("view.toolbar_focus"),
            Sub(
                "Panels",
                &[
                    C("view.workspace"),
                    C("view.documents"),
                    C("view.outline"),
                    C("view.documentMap"),
                    Sep,
                    C("view.bottom_panel.search"),
                    C("view.bottom_panel.compare"),
                    C("view.bottom_panel.output"),
                    C("view.bottom_panel.close"),
                ],
            ),
            Sub(
                "Workspace",
                &[
                    C("workspace.refresh"),
                    C("workspace.loadMore"),
                    Sep,
                    C("workspace.createFile"),
                    C("workspace.createFolder"),
                    C("workspace.rename"),
                    C("workspace.delete"),
                ],
            ),
            Sub(
                "Document List",
                &[
                    C("documents.sortName"),
                    C("documents.sortPath"),
                    C("documents.sortTabOrder"),
                    Sep,
                    C("documents.save"),
                    C("documents.close"),
                ],
            ),
            Sep,
            Sub(
                "Fold",
                &[
                    C("view.fold.all"),
                    C("view.fold.unfoldAll"),
                    C("view.fold.toggleCurrent"),
                    Sep,
                    C("view.fold.level1"),
                    C("view.fold.level2"),
                    C("view.fold.level3"),
                    C("view.fold.level4"),
                    C("view.fold.level5"),
                    C("view.fold.level6"),
                    C("view.fold.level7"),
                    C("view.fold.level8"),
                ],
            ),
            Sep,
            Sub(
                "Split",
                &[
                    C("view.split_vertical"),
                    C("view.split_horizontal"),
                    Sep,
                    C("view.clone_other"),
                    C("view.move_other"),
                    C("view.close_split"),
                    C("view.focus_other"),
                    Sep,
                    C("view.sync_vertical"),
                    C("view.sync_horizontal"),
                ],
            ),
            Sub(
                "Tabs",
                &[
                    C("view.tabs.vertical"),
                    C("view.tabs.pin"),
                    C("view.tabs.color"),
                    Sep,
                    C("view.tabs.move_left"),
                    C("view.tabs.move_right"),
                    Sep,
                    C("view.tabs.sort_name"),
                    C("view.tabs.sort_path"),
                    C("view.tabs.sort_descending"),
                ],
            ),
            Sep,
            C("view.theme.cycle"),
            C("view.command_palette"),
        ],
    ),
    Sub(
        "Encoding",
        &[
            C("encoding.charsets"),
            Sep,
            C("encoding.bom_on"),
            C("encoding.bom_off"),
            // Convert To ▸, Interpret As ▸, Line Endings ▸ and Binary Warning ▸ are
            // filled from the codec commands' declared menu paths.
        ],
    ),
    Sub(
        "Language",
        &[
            C("language.choose"),
            Sep,
            Sub(
                "User-Defined",
                &[
                    C("language.udl.import"),
                    C("language.udl.edit"),
                    C("language.udl.export"),
                    C("language.udl.preview"),
                ],
            ),
            Sub(
                "Outline Definitions",
                &[
                    C("outline.loadDefinition"),
                    C("outline.importFunctionList"),
                    C("outline.exportDefinition"),
                    C("outline.cancelImport"),
                ],
            ),
            Sep,
            C("editor.completion.show"),
            C("language.signatures.import"),
        ],
    ),
    Sub(
        "Settings",
        &[
            C("settings.open"),
            C("settings.shortcuts"),
            Sep,
            C("settings.keymap_import"),
            C("settings.keymap_export"),
            C("settings.keymap_open"),
            Sep,
            Sub(
                "Import from Notepad++",
                &[
                    C("migration.review"),
                    C("migration.apply"),
                    C("migration.open_paths"),
                    C("migration.cancel"),
                ],
            ),
        ],
    ),
    Sub(
        "Macro",
        &[
            C("macro.record"),
            C("macro.stop"),
            Sep,
            C("macro.play"),
            C("macro.play_n"),
            C("macro.play_eof"),
            C("macro.cancel"),
            C("macro.resume"),
            Sep,
            C("macro.manager"),
            C("macro.rename"),
            C("macro.shortcut"),
            C("macro.ghost"),
            C("macro.manager_close"),
            Sep,
            C("macro.save"),
            C("macro.reload"),
            C("macro.import"),
            C("macro.export"),
        ],
    ),
    Sub(
        "Run",
        &[
            C("run.prompt"),
            C("run.execute"),
            Sep,
            C("run.load"),
            C("run.cancel"),
            Sep,
            Sub(
                "Output",
                &[
                    C("output.clear"),
                    C("output.copy"),
                    C("output.save"),
                    C("output.close"),
                    C("output.open_link"),
                ],
            ),
        ],
    ),
    Sub(
        "Tools",
        &[
            Sub(
                "Utilities",
                &[
                    C("utilities.md5"),
                    C("utilities.sha1"),
                    C("utilities.sha256"),
                    C("utilities.sha512"),
                    Sep,
                    C("utilities.base64Encode"),
                    C("utilities.base64Decode"),
                    C("utilities.urlEncode"),
                    C("utilities.urlDecode"),
                    Sep,
                    C("utilities.statistics"),
                    C("utilities.cancel"),
                ],
            ),
            Sub("Export", &[C("utilities.exportHtml"), C("utilities.exportRtf")]),
            Sub(
                "Compare",
                &[
                    C("compare.open"),
                    C("compare.recompare"),
                    C("compare.cancel"),
                    C("compare.close"),
                    Sep,
                    C("compare.next"),
                    C("compare.previous"),
                    Sep,
                    C("compare.copyLeftToRight"),
                    C("compare.copyRightToLeft"),
                    C("compare.copySelectionLeftToRight"),
                    C("compare.copySelectionRightToLeft"),
                    Sep,
                    C("compare.swap"),
                    C("compare.leftSource"),
                    C("compare.rightSource"),
                    Sep,
                    C("compare.disk"),
                    C("compare.lastSaved"),
                    C("compare.external"),
                    Sep,
                    Sub(
                        "Compare Options",
                        &[
                            C("compare.options"),
                            Sep,
                            C("compare.whitespace"),
                            C("compare.trimEdges"),
                            C("compare.ignoreWhitespace"),
                            C("compare.ignoreBlank"),
                            C("compare.ignoreCase"),
                            C("compare.ignoreEol"),
                            C("compare.ignoreBom"),
                            C("compare.normalizeTabs"),
                            Sep,
                            C("compare.syncHorizontal"),
                            C("compare.pauseAutomatic"),
                        ],
                    ),
                    Sub(
                        "Compare Colors",
                        &[
                            C("compare.colorAdded"),
                            C("compare.accentAdded"),
                            C("compare.gutterAdded"),
                            C("compare.resetAdded"),
                            Sep,
                            C("compare.colorRemoved"),
                            C("compare.accentRemoved"),
                            C("compare.gutterRemoved"),
                            C("compare.resetRemoved"),
                            Sep,
                            C("compare.colorChanged"),
                            C("compare.accentChanged"),
                            C("compare.gutterChanged"),
                            C("compare.resetChanged"),
                            Sep,
                            C("compare.colorMoved"),
                            C("compare.accentMoved"),
                            C("compare.gutterMoved"),
                            C("compare.resetMoved"),
                            Sep,
                            C("compare.colorCurrent"),
                            C("compare.accentCurrent"),
                            C("compare.gutterCurrent"),
                            C("compare.resetCurrent"),
                            Sep,
                            C("compare.colorblind"),
                            C("compare.defaults"),
                            Sep,
                            C("compare.themeLight"),
                            C("compare.themeDark"),
                            C("compare.themeSystem"),
                        ],
                    ),
                ],
            ),
            Sub(
                "Extensions",
                &[
                    C("extensions.manage"),
                    Sep,
                    C("ext.json.format"),
                    C("ext.json.minify"),
                    C("ext.json.validate"),
                    C("ext.json.tree"),
                    C("ext.xml.format"),
                    C("ext.xml.validate"),
                    C("ext.xml.xpath"),
                    C("ext.hex.open"),
                ],
            ),
        ],
    ),
    Sub(
        "Window",
        &[
            C("view.tabs.previous"),
            C("view.tabs.next"),
            C("view.tabs.mru"),
            Sep,
            C("window.select.0"),
            C("window.select.1"),
            C("window.select.2"),
            C("window.select.3"),
            C("window.select.4"),
            C("window.select.5"),
            C("window.select.6"),
            C("window.select.7"),
            C("window.select.8"),
            C("window.select.9"),
            C("window.select.10"),
            C("window.select.11"),
            C("window.select.12"),
            C("window.select.13"),
            C("window.select.14"),
            C("window.select.15"),
            C("window.select.16"),
            C("window.select.17"),
            C("window.select.18"),
            C("window.select.19"),
            Sep,
            Sub("Tray", &[C("tray.toggle"), C("tray.hide"), C("tray.restore")]),
        ],
    ),
    Sub(
        "Help",
        &[
            C("update.check"),
            C("update.apply_on_exit"),
            C("update.discard"),
            C("update.cancel"),
            Sep,
            C("help.about"),
        ],
    ),
];

/// Every command ID [`TREE`] names, in order.
pub fn tree_command_ids() -> Vec<&'static str> {
    fn walk(items: &[MenuTemplate], out: &mut Vec<&'static str>) {
        for item in items {
            match *item {
                MenuTemplate::Command(id) => out.push(id),
                MenuTemplate::Submenu(_, children) => walk(children, out),
                MenuTemplate::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(TREE, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_commands::{CommandId, MenuItem};

    /// A registry close to what the shell assembles: the built-ins plus the
    /// feature commands registered inside this crate.
    fn registry() -> CommandRegistry {
        let mut registry = bareline_commands::shell_commands();
        crate::search_panel::register_commands(&mut registry).unwrap();
        bareline_editor_surface::power::register_commands(&mut registry);
        crate::language::register_commands(&mut registry);
        crate::encoding::register(&mut registry);
        crate::macros::register_commands(&mut registry);
        crate::workspace_panel::register_commands(&mut registry);
        crate::compare::register_commands(&mut registry);
        crate::utilities::register_commands(&mut registry);
        apply_menu_placement(&mut registry);
        registry
    }

    fn palette_only(registry: &CommandRegistry, id: CommandId) -> bool {
        registry
            .presentation(id)
            .is_some_and(|meta| meta.menu == MenuPlacement::PaletteOnly)
    }

    fn collect(items: &[MenuItem], out: &mut std::collections::BTreeSet<CommandId>) {
        for item in items {
            match item {
                MenuItem::Command(id) => {
                    out.insert(*id);
                }
                MenuItem::Submenu { items, .. } => collect(items, out),
                MenuItem::Separator => {}
            }
        }
    }

    #[test]
    fn every_non_internal_command_is_reachable_and_internal_stays_hidden() {
        let registry = registry();
        let model = curated_model(&registry);
        // Every top-level menu is a taxonomy category, kept in taxonomy order, and
        // the "Other" catch-all is nested under Tools rather than sitting at the
        // top level. (Menus with no commands in this crate-only registry — e.g.
        // Settings and Window, whose commands register in the app binary — simply
        // do not appear here.)
        let tops: Vec<&str> = model
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Submenu { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert!(!tops.contains(&"Other"), "Other must nest under Tools");
        let mut taxonomy = bareline_commands::MENU_TAXONOMY.iter();
        for top in &tops {
            assert!(
                taxonomy.any(|name| name == top),
                "unexpected or out-of-order top-level menu {top:?}"
            );
        }
        for expected in ["File", "Edit", "Search", "View", "Encoding", "Tools"] {
            assert!(tops.contains(&expected), "{expected} menu missing");
        }

        let mut reachable = std::collections::BTreeSet::new();
        collect(&model.items, &mut reachable);
        for spec in registry.entries() {
            let internal = registry.presentation(spec.id).is_some_and(|meta| meta.internal);
            if internal || palette_only(&registry, spec.id) {
                assert!(
                    !reachable.contains(&spec.id),
                    "internal or palette-only command {} leaked into the menus",
                    spec.id.0
                );
            } else {
                assert!(
                    reachable.contains(&spec.id),
                    "non-internal command {} is unreachable from the curated menus",
                    spec.id.0
                );
            }
        }
    }

    #[test]
    fn codec_submenus_are_populated_by_declared_paths() {
        let registry = registry();
        let model = curated_model(&registry);
        let encoding = model
            .items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu { title, items } if title == "Encoding" => Some(items),
                _ => None,
            })
            .expect("Encoding menu present");
        let has_convert = encoding
            .iter()
            .any(|item| matches!(item, MenuItem::Submenu { title, .. } if title == "Convert To"));
        assert!(has_convert, "Convert To submenu auto-populated from codec paths");
    }

    #[test]
    fn tree_names_each_command_once_and_placements_name_tree_commands() {
        let ids = tree_command_ids();
        let mut seen = std::collections::BTreeSet::new();
        for id in &ids {
            assert!(seen.insert(*id), "{id} appears twice in the menu tree");
        }
        for id in WHEN_ENABLED.iter().chain(PALETTE_ONLY) {
            assert!(
                seen.contains(id) || id.starts_with("encoding."),
                "{id} has a menu placement but no place in the tree"
            );
        }
    }

    #[test]
    fn crate_commands_have_a_home_and_human_titles() {
        let registry = registry();
        let model = curated_model(&registry);
        let other = model.items.iter().find_map(|item| match item {
            MenuItem::Submenu { title, items } if title == "Tools" => items.iter().find_map(|item| match item {
                MenuItem::Submenu { title, items } if title == bareline_commands::OTHER_MENU => Some(items),
                _ => None,
            }),
            _ => None,
        });
        let mut homeless = std::collections::BTreeSet::new();
        if let Some(items) = other {
            collect(items, &mut homeless);
        }
        assert!(homeless.is_empty(), "commands without a menu home: {homeless:?}");
        for spec in registry.entries() {
            assert!(
                !bareline_commands::title_looks_like_identifier(spec.title),
                "{} has an identifier-like title {:?}",
                spec.id.0,
                spec.title
            );
        }
    }
}
