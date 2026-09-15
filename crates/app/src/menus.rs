// SPDX-License-Identifier: MPL-2.0
//! The hand-authored menu taxonomy. Instead of emitting menu items in
//! registration order (which produced a File menu that opened with *Exit* and an
//! Edit menu taller than the screen), the twelve top-level menus are laid out
//! here as ordered trees with separators and submenus.
//!
//! Commands the tree does not place explicitly are auto-routed by their declared
//! menu path (see [`bareline_commands::MenuModel::curated`]), so a newly
//! registered command still lands somewhere sensible — and anything with no home
//! at all falls into Tools ▸ Other rather than disappearing. Command IDs listed
//! here that are not registered (optional features) are simply skipped.

use bareline_commands::MenuTemplate::{Command as C, Separator as Sep, Submenu as Sub};
use bareline_commands::{CommandRegistry, MenuModel, MenuTemplate};

/// Build the live menu model for `registry` from [`TREE`].
pub fn curated_model(registry: &CommandRegistry) -> MenuModel {
    MenuModel::curated(registry, TREE)
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
            Sub(
                "Output",
                &[C("file.save_copy"), C("file.save_all"), C("utilities.print")],
            ),
            Sep,
            C("file.close"),
            Sub(
                "Document and Disk",
                &[
                    C("workspace.rename"),
                    C("file.restore_closed"),
                    C("file.read_only"),
                    C("file.reveal"),
                    C("file.terminal"),
                    Sep,
                    C("file.external.reload"),
                    C("file.cancel_operations"),
                    C("file.transcode.resume"),
                    C("file.transcode.cancel"),
                    C("file.cancel_save_all"),
                    Sep,
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
                            Sep,
                            C("file.external.check"),
                            C("file.external.auto_reload"),
                            C("file.external.keep"),
                        ],
                    ),
                ],
            ),
            Sub("Tray", &[C("tray.toggle"), C("tray.hide"), C("tray.restore")]),
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
                    C("editor.rectangle.select"),
                    C("editor.rectangle.copy"),
                    C("editor.rectangle.cut"),
                    C("editor.rectangle.paste"),
                    C("editor.rectangle.delete"),
                    C("editor.rectangle.extend"),
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
            Sub("Options", &[C("search.match_case"), C("search.whole_word")]),
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
            Sub(
                "Panels",
                &[
                    C("view.workspace"),
                    C("view.documents"),
                    C("view.outline"),
                    C("view.documentMap"),
                ],
            ),
            Sep,
            C("editor.wrap.mode"),
            C("editor.render.whitespace"),
            C("editor.line_numbers"),
            C("editor.minimap"),
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
        ],
    ),
    Sub(
        "Macro",
        &[
            C("macro.record"),
            C("macro.play"),
            C("macro.play_n"),
            Sep,
            C("macro.manager"),
            C("macro.save"),
            C("macro.import"),
            C("macro.export"),
        ],
    ),
    Sub(
        "Run",
        &[C("run.prompt"), C("run.execute"), Sep, C("run.load"), C("run.cancel")],
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
                ],
            ),
            Sub("Export", &[C("utilities.exportHtml"), C("utilities.exportRtf")]),
            Sub(
                "Extensions",
                &[
                    C("extensions.manage"),
                    C("extensions.catalog"),
                    C("extensions.install"),
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
            // Compare ▸ is filled from the compare commands' "Tools > Compare" paths.
        ],
    ),
    Sub(
        "Window",
        &[
            C("view.tabs.previous"),
            C("view.tabs.next"),
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
            C("view.tabs.mru"),
        ],
    ),
    Sub("Help", &[C("update.check"), Sep, C("help.about")]),
];

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
        registry
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
            if internal {
                assert!(
                    !reachable.contains(&spec.id),
                    "internal command {} leaked into the menus",
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
}
