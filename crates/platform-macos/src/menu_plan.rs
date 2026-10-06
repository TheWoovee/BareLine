// SPDX-License-Identifier: MPL-2.0
//! The macOS menu bar as plain data: which items exist, in which menus, with
//! which tags, key equivalents, titles and states. `MacMenuBar` turns a plan
//! into `NSMenu`/`NSMenuItem` objects; everything decided here is tested
//! without AppKit.
//!
//! The layout follows the Human Interface Guidelines. An application menu comes
//! first, with About, Settings…, Services, Hide, Hide Others, Show All and
//! Quit; About, Settings and Quit are Bareline's own commands, moved there from
//! the Help, Settings and File menus. The shell's curated menus follow in model
//! order. A command item's tag is its 1-based position in [`MenuPlan`], the
//! same numbering the Windows adapter uses for menu ids, so a tag travels as
//! `CommandMessage::id` and maps back through [`MenuPlan::command_id`] and
//! [`MenuPlan::action`].
use crate::keys::{KeyEquivalent, ModifierMask, key_equivalent};
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, Keymap, MenuItem, MenuModel, OTHER_MENU};
use std::collections::BTreeMap;

pub const ABOUT_COMMAND: CommandId = CommandId("help.about");
pub const SETTINGS_COMMAND: CommandId = CommandId("settings.open");
pub const QUIT_COMMAND: CommandId = CommandId("app.quit");
/// Bareline commands the application menu hosts, with their HIG titles and
/// fixed key equivalents (none for About, ⌘, and ⌘Q).
const APPLICATION_COMMANDS: [(CommandId, &str, Option<&str>); 3] = [
    (ABOUT_COMMAND, "About Bareline", None),
    (SETTINGS_COMMAND, "Settings\u{2026}", Some(",")),
    (QUIT_COMMAND, "Quit Bareline", Some("q")),
];
/// Edit commands sent as the standard AppKit actions along the responder chain,
/// so the text fields of open and save panels and alerts keep Undo, Cut, Copy,
/// Paste and Select All. In the editor window Bareline's responder answers
/// them with the item's command.
pub const STANDARD_EDIT_ACTIONS: [(CommandId, &str); 6] = [
    (CommandId("edit.undo"), "undo:"),
    (CommandId("edit.redo"), "redo:"),
    (CommandId("edit.cut"), "cut:"),
    (CommandId("edit.copy"), "copy:"),
    (CommandId("edit.paste"), "paste:"),
    (CommandId("edit.select_all"), "selectAll:"),
];
/// Shortcuts macOS keeps for itself: the application menu's own (⌘, ⌘H ⌥⌘H
/// ⌘Q) and the system's (⌘Tab, ⌘Space, ⌘`). A keymap binding that lands on one
/// keeps working as a key press but is not shown on, or claimed by, a menu item.
fn reserved() -> [KeyEquivalent; 7] {
    let command = ModifierMask::COMMAND;
    [
        KeyEquivalent::new(",", command),
        KeyEquivalent::new("h", command),
        KeyEquivalent::new("h", command.union(ModifierMask::OPTION)),
        KeyEquivalent::new("q", command),
        KeyEquivalent::new("\t", command),
        KeyEquivalent::new(" ", command),
        KeyEquivalent::new("`", command),
    ]
}

/// A chosen menu command, in the shape of the Windows adapter's so the seam
/// handles both alike: `hwnd` is the window token the menu bar was created
/// for (the seam's `RawWindow`), `id` the item's tag and `action` 0 (macOS
/// menus have no per-item right-click actions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMessage {
    pub hwnd: isize,
    pub id: usize,
    pub action: u16,
}
impl CommandMessage {
    /// The window token the command was sent to. The shell's seam names it
    /// through this accessor: its portability guard keeps Win32 field names
    /// out of the shell.
    pub fn window(&self) -> isize {
        self.hwnd
    }
}

/// How an item reaches its handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    /// Bareline's menu target, which queues a `CommandMessage` for the tag.
    Dispatch,
    /// A standard action (`copy:`) sent to the first responder; see
    /// [`STANDARD_EDIT_ACTIONS`].
    Responder(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanCommand {
    pub tag: isize,
    pub command: CommandId,
    pub action: ItemAction,
    /// The HIG title of an application-menu command; its localization key is
    /// `macos.<command id>`.
    pub fixed_title: Option<&'static str>,
}

/// An item NSApplication handles itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemItem {
    /// English title; its localization key is `macos.<selector>`.
    pub title: &'static str,
    pub selector: &'static str,
    pub key: Option<KeyEquivalent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanItem {
    Command(PlanCommand),
    System(SystemItem),
    Separator,
    Submenu(PlanMenu),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    /// The first menu, titled with the application's name by macOS.
    Application,
    /// Filled by macOS (`NSApplication.servicesMenu`).
    Services,
    /// Gets the system's Help search field (`NSApplication.helpMenu`).
    Help,
    Ordinary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanMenu {
    /// English title; its localization key is `menu.<title>`, as on Windows.
    pub title: String,
    pub role: MenuRole,
    pub items: Vec<PlanItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuPlan {
    pub menus: Vec<PlanMenu>,
    commands: Vec<CommandId>,
    actions: Vec<Action>,
}

/// One command item's projected title, state and key equivalent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemState {
    pub tag: isize,
    pub title: String,
    pub enabled: bool,
    pub checked: bool,
    /// AppKit draws a check mark for radio items too; kept for parity.
    pub radio: bool,
    pub key: Option<KeyEquivalent>,
}

/// Every title and state the menu bar shows, in [`MenuPlan::walk`] order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuProjection {
    pub items: Vec<ItemState>,
    /// Titles of the menus and submenus.
    pub menus: Vec<String>,
    /// Titles of the system items (Hide, Hide Others, Show All).
    pub system: Vec<String>,
}

/// One node of a pre-order walk over a plan.
pub enum PlanNode<'a> {
    Menu(&'a PlanMenu),
    Command(&'a PlanCommand),
    System(&'a SystemItem),
}

fn has_entries(items: &[PlanItem]) -> bool {
    items.iter().any(|item| !matches!(item, PlanItem::Separator))
}
/// Drops leading, doubled and trailing separators.
fn tidy(items: Vec<PlanItem>) -> Vec<PlanItem> {
    let mut out: Vec<PlanItem> = Vec::with_capacity(items.len());
    for item in items {
        if matches!(item, PlanItem::Separator) && matches!(out.last(), None | Some(PlanItem::Separator)) {
            continue;
        }
        out.push(item);
    }
    while matches!(out.last(), Some(PlanItem::Separator)) {
        out.pop();
    }
    out
}

/// A title without Windows access-key markers: `&Save` reads `Save` and an
/// escaped `&&` stays one ampersand. macOS menus have no mnemonics.
pub fn plain_title(title: &str) -> String {
    let mut plain = String::with_capacity(title.len());
    let mut characters = title.chars();
    while let Some(character) = characters.next() {
        if character == '&' {
            if let Some(next) = characters.next() {
                plain.push(next);
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

impl MenuPlan {
    /// The plan for a menu model, normally `MenuModel::visible`. Internal and
    /// unregistered commands are left out, as on Windows.
    pub fn build(model: &MenuModel, registry: &CommandRegistry) -> Self {
        let mut plan = Self::default();
        let application = plan.application_menu(registry);
        let mut menus = vec![application];
        let mut loose = Vec::new();
        for item in &model.items {
            match item {
                MenuItem::Submenu { title, items } => {
                    let items = plan.items(items, registry);
                    if has_entries(&items) {
                        let role = if title == "Help" {
                            MenuRole::Help
                        } else {
                            MenuRole::Ordinary
                        };
                        menus.push(PlanMenu {
                            title: title.clone(),
                            role,
                            items,
                        });
                    }
                }
                // The menu bar holds only menus; stray top-level commands are
                // collected into one at the end instead of being dropped.
                other => loose.push(other.clone()),
            }
        }
        let loose = plan.items(&loose, registry);
        if has_entries(&loose) {
            menus.push(PlanMenu {
                title: OTHER_MENU.into(),
                role: MenuRole::Ordinary,
                items: loose,
            });
        }
        plan.menus = menus;
        plan
    }

    fn register(
        &mut self,
        registry: &CommandRegistry,
        id: CommandId,
        fixed_title: Option<&'static str>,
    ) -> Option<PlanCommand> {
        if registry.presentation(id).is_some_and(|metadata| metadata.internal) {
            return None;
        }
        let spec = registry.spec(id)?;
        self.commands.push(id);
        self.actions.push(spec.action);
        let action = STANDARD_EDIT_ACTIONS
            .iter()
            .find(|(standard, _)| *standard == id)
            .map_or(ItemAction::Dispatch, |(_, selector)| ItemAction::Responder(selector));
        Some(PlanCommand {
            tag: self.commands.len() as isize,
            command: id,
            action,
            fixed_title,
        })
    }

    fn application_menu(&mut self, registry: &CommandRegistry) -> PlanMenu {
        let command = |plan: &mut Self, index: usize| {
            let (id, title, _) = APPLICATION_COMMANDS[index];
            plan.register(registry, id, Some(title)).map(PlanItem::Command)
        };
        let system =
            |title, selector, key: Option<KeyEquivalent>| PlanItem::System(SystemItem { title, selector, key });
        let command_key = ModifierMask::COMMAND;
        let mut items = Vec::new();
        items.extend(command(self, 0));
        items.push(PlanItem::Separator);
        items.extend(command(self, 1));
        items.push(PlanItem::Separator);
        items.push(PlanItem::Submenu(PlanMenu {
            title: "Services".into(),
            role: MenuRole::Services,
            items: Vec::new(),
        }));
        items.push(PlanItem::Separator);
        items.push(system(
            "Hide Bareline",
            "hide:",
            Some(KeyEquivalent::new("h", command_key)),
        ));
        items.push(system(
            "Hide Others",
            "hideOtherApplications:",
            Some(KeyEquivalent::new("h", command_key.union(ModifierMask::OPTION))),
        ));
        items.push(system("Show All", "unhideAllApplications:", None));
        items.push(PlanItem::Separator);
        items.extend(command(self, 2));
        PlanMenu {
            title: "Bareline".into(),
            role: MenuRole::Application,
            items: tidy(items),
        }
    }

    fn items(&mut self, items: &[MenuItem], registry: &CommandRegistry) -> Vec<PlanItem> {
        let mut out = Vec::new();
        for item in items {
            match item {
                MenuItem::Separator => out.push(PlanItem::Separator),
                MenuItem::Command(id) => {
                    // About, Settings and Quit live in the application menu.
                    if APPLICATION_COMMANDS.iter().any(|(hosted, _, _)| hosted == id) {
                        continue;
                    }
                    out.extend(self.register(registry, *id, None).map(PlanItem::Command));
                }
                MenuItem::Submenu { title, items } => {
                    let children = self.items(items, registry);
                    if has_entries(&children) {
                        out.push(PlanItem::Submenu(PlanMenu {
                            title: title.clone(),
                            role: MenuRole::Ordinary,
                            items: children,
                        }));
                    }
                }
            }
        }
        tidy(out)
    }

    /// The command behind a tag (`CommandMessage::id`); `None` for 0 or an
    /// unknown tag.
    pub fn command_id(&self, tag: usize) -> Option<CommandId> {
        tag.checked_sub(1).and_then(|index| self.commands.get(index)).copied()
    }
    pub fn action(&self, tag: usize) -> Option<Action> {
        tag.checked_sub(1).and_then(|index| self.actions.get(index)).copied()
    }
    pub fn tag(&self, command: CommandId) -> Option<isize> {
        self.commands
            .iter()
            .position(|candidate| *candidate == command)
            .map(|index| index as isize + 1)
    }
    pub fn len(&self) -> usize {
        self.commands.len()
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Visits every menu, command item and system item in pre-order: a menu
    /// before its items. The AppKit builder and [`Self::project`] use this one
    /// order, so projections line up with the native objects.
    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(PlanNode<'a>)) {
        fn items<'a>(entries: &'a [PlanItem], visit: &mut impl FnMut(PlanNode<'a>)) {
            for item in entries {
                match item {
                    PlanItem::Command(command) => visit(PlanNode::Command(command)),
                    PlanItem::System(system) => visit(PlanNode::System(system)),
                    PlanItem::Separator => {}
                    PlanItem::Submenu(menu) => {
                        visit(PlanNode::Menu(menu));
                        items(&menu.items, visit);
                    }
                }
            }
        }
        for menu in &self.menus {
            visit(PlanNode::Menu(menu));
            items(&menu.items, visit);
        }
    }

    /// Key equivalents by tag. The application menu's own are fixed; every
    /// other command takes its first single-chord keymap binding that maps to
    /// a key equivalent (see [`key_equivalent`]) and is neither reserved by
    /// macOS nor already taken by an earlier item in menu order.
    pub fn key_equivalents(&self, keymap: &Keymap) -> BTreeMap<isize, KeyEquivalent> {
        let mut taken: Vec<KeyEquivalent> = reserved().to_vec();
        let mut keys = BTreeMap::new();
        let mut commands = Vec::new();
        self.walk(&mut |node| {
            if let PlanNode::Command(command) = node {
                commands.push(command);
            }
        });
        for command in commands {
            if let Some((_, _, fixed)) = APPLICATION_COMMANDS.iter().find(|(id, _, _)| *id == command.command) {
                if let Some(key) = fixed {
                    keys.insert(command.tag, KeyEquivalent::new(*key, ModifierMask::COMMAND));
                }
                continue;
            }
            let candidate = keymap
                .bindings()
                .iter()
                .filter(|binding| binding.command == command.command)
                .filter_map(|binding| match binding.sequence.as_slice() {
                    [chord] => key_equivalent(chord),
                    // A key sequence (Ctrl+K Ctrl+C) has no menu form.
                    _ => None,
                })
                .find(|key| !taken.contains(key));
            if let Some(key) = candidate {
                taken.push(key.clone());
                keys.insert(command.tag, key);
            }
        }
        keys
    }

    /// Titles, states and key equivalents for the current context, keymap and
    /// locale. `label_for(id, fallback)` is the shell's resolver: command ids,
    /// `menu.<English title>` and `macos.<id or selector>` keys. A state label is
    /// live data (a document name, a recent file) and is shown as it is.
    pub fn project(
        &self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
        label_for: impl Fn(&str, &str) -> String,
    ) -> MenuProjection {
        let keys = self.key_equivalents(keymap);
        let mut projection = MenuProjection::default();
        self.walk(&mut |node| match node {
            PlanNode::Menu(menu) => projection
                .menus
                .push(plain_title(&label_for(&format!("menu.{}", menu.title), &menu.title))),
            PlanNode::System(system) => projection
                .system
                .push(label_for(&format!("macos.{}", system.selector), system.title)),
            PlanNode::Command(command) => {
                let (Some(spec), Some(state)) =
                    (registry.spec(command.command), registry.state(command.command, context))
                else {
                    return;
                };
                let title = match (&state.label, command.fixed_title) {
                    (Some(label), _) => label.clone(),
                    (None, Some(fixed)) => label_for(&format!("macos.{}", command.command.0), fixed),
                    (None, None) => plain_title(&label_for(command.command.0, spec.title)),
                };
                projection.items.push(ItemState {
                    tag: command.tag,
                    title,
                    enabled: state.enabled,
                    checked: state.checked,
                    radio: state.radio,
                    key: keys.get(&command.tag).cloned(),
                });
            }
        });
        projection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_commands::{CommandPresentation, CommandSpec, CommandState, KeyBinding, KeyChord};

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::default();
        for (id, title, shortcut, action) in [
            ("file.new", "&New", "Ctrl+N", Action::New),
            ("file.save", "Save", "Ctrl+S", Action::Save),
            ("app.quit", "Exit", "Alt+F4", Action::Quit),
            ("edit.copy", "Copy", "Ctrl+C", Action::Copy),
            ("edit.paste", "Paste", "Ctrl+V", Action::Paste),
            ("search.replace", "Replace", "Ctrl+H", Action::Replace),
            ("search.find", "Find", "Ctrl+F", Action::Find),
            ("search.find_next", "Find Next", "F3", Action::FindNext),
            (
                "settings.open",
                "Settings",
                "Ctrl+,",
                Action::Contributed(CommandId("settings.open")),
            ),
            ("help.about", "About Bareline", "F1", Action::About),
            ("debug.inspect", "Inspect", "", Action::Palette),
        ] {
            registry
                .register(CommandSpec {
                    id: CommandId(id),
                    title,
                    category: "Test",
                    shortcut,
                    action,
                })
                .unwrap();
        }
        registry
            .set_presentation(
                CommandId("debug.inspect"),
                CommandPresentation {
                    internal: true,
                    ..CommandPresentation::default()
                },
            )
            .unwrap();
        registry
    }

    fn model() -> MenuModel {
        use MenuItem::{Command, Separator, Submenu};
        let id = |name| Command(CommandId(name));
        MenuModel {
            items: vec![
                Submenu {
                    title: "File".into(),
                    items: vec![id("file.new"), id("file.save"), Separator, id("app.quit")],
                },
                Submenu {
                    title: "Edit".into(),
                    items: vec![id("edit.copy"), id("edit.paste"), Separator, id("debug.inspect")],
                },
                Submenu {
                    title: "Search".into(),
                    items: vec![
                        id("search.find"),
                        id("search.find_next"),
                        Submenu {
                            title: "More".into(),
                            items: vec![id("search.replace")],
                        },
                    ],
                },
                Submenu {
                    title: "Settings".into(),
                    items: vec![id("settings.open")],
                },
                Submenu {
                    title: "Help".into(),
                    items: vec![Separator, id("help.about")],
                },
                id("file.new"),
            ],
        }
    }

    fn titles(menu: &PlanMenu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                PlanItem::Command(command) => command.command.0.to_owned(),
                PlanItem::System(system) => system.selector.to_owned(),
                PlanItem::Separator => "-".to_owned(),
                PlanItem::Submenu(menu) => format!("[{}]", menu.title),
            })
            .collect()
    }

    #[test]
    fn application_menu_hosts_about_settings_and_quit() {
        let plan = MenuPlan::build(&model(), &registry());
        let application = &plan.menus[0];
        assert_eq!(application.role, MenuRole::Application);
        assert_eq!(
            titles(application),
            [
                "help.about",
                "-",
                "settings.open",
                "-",
                "[Services]",
                "-",
                "hide:",
                "hideOtherApplications:",
                "unhideAllApplications:",
                "-",
                "app.quit"
            ]
        );
        // Moved commands leave their menus; emptied menus and dangling
        // separators go with them.
        let names: Vec<_> = plan.menus.iter().map(|menu| menu.title.as_str()).collect();
        assert_eq!(names, ["Bareline", "File", "Edit", "Search", "Other"]);
        assert_eq!(titles(&plan.menus[1]), ["file.new", "file.save"]);
        // The internal command is not listed, and neither is its separator.
        assert_eq!(titles(&plan.menus[2]), ["edit.copy", "edit.paste"]);
        assert_eq!(titles(&plan.menus[3]), ["search.find", "search.find_next", "[More]"]);
        // A stray top-level command is kept in a menu of its own.
        assert_eq!(titles(&plan.menus[4]), ["file.new"]);
    }

    #[test]
    fn help_menu_gets_the_help_role_when_it_keeps_entries() {
        let mut model = model();
        if let MenuItem::Submenu { items, .. } = &mut model.items[4] {
            items.push(MenuItem::Command(CommandId("search.find")));
        }
        let plan = MenuPlan::build(&model, &registry());
        let help = plan.menus.iter().find(|menu| menu.title == "Help").unwrap();
        assert_eq!(help.role, MenuRole::Help);
        assert_eq!(titles(help), ["search.find"]);
    }

    #[test]
    fn tags_map_back_to_commands_and_actions() {
        let plan = MenuPlan::build(&model(), &registry());
        let mut seen = Vec::new();
        plan.walk(&mut |node| {
            if let PlanNode::Command(command) = node {
                seen.push(command.tag);
                assert_eq!(plan.command_id(command.tag as usize), Some(command.command));
            }
        });
        // Tags are 1-based and unique, even for a command listed twice.
        let mut unique = seen.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), seen.len());
        assert_eq!(unique, (1..=plan.len() as isize).collect::<Vec<_>>());
        assert_eq!(plan.command_id(0), None);
        assert_eq!(plan.command_id(plan.len() + 1), None);
        let save = plan.tag(CommandId("file.save")).unwrap() as usize;
        assert_eq!(plan.action(save), Some(Action::Save));
        let quit = plan.tag(QUIT_COMMAND).unwrap() as usize;
        assert_eq!(plan.action(quit), Some(Action::Quit));
    }

    #[test]
    fn standard_edit_commands_use_responder_actions() {
        let plan = MenuPlan::build(&model(), &registry());
        let mut actions = BTreeMap::new();
        plan.walk(&mut |node| {
            if let PlanNode::Command(command) = node {
                actions.insert(command.command.0, command.action);
            }
        });
        assert_eq!(actions["edit.copy"], ItemAction::Responder("copy:"));
        assert_eq!(actions["edit.paste"], ItemAction::Responder("paste:"));
        assert_eq!(actions["file.save"], ItemAction::Dispatch);
        assert_eq!(actions["app.quit"], ItemAction::Dispatch);
    }

    #[test]
    fn key_equivalents_come_from_the_keymap_and_yield_to_macos() {
        let registry = registry();
        let plan = MenuPlan::build(&model(), &registry);
        let keys = plan.key_equivalents(&Keymap::defaults(&registry));
        let key = |id| keys.get(&plan.tag(CommandId(id)).unwrap()).cloned();
        let command = ModifierMask::COMMAND;
        assert_eq!(key("file.save"), Some(KeyEquivalent::new("s", command)));
        assert_eq!(key("edit.copy"), Some(KeyEquivalent::new("c", command)));
        assert_eq!(
            key("search.find_next"),
            Some(KeyEquivalent::new("\u{f706}", ModifierMask::NONE))
        );
        // The application menu's own shortcuts replace the Windows bindings.
        assert_eq!(key("app.quit"), Some(KeyEquivalent::new("q", command)));
        assert_eq!(key("settings.open"), Some(KeyEquivalent::new(",", command)));
        assert_eq!(key("help.about"), None);
        // Ctrl+H would be ⌘H, which hides the application on a Mac.
        assert_eq!(key("search.replace"), None);
    }

    #[test]
    fn reserved_keys_sequences_and_repeated_items_get_no_equivalent() {
        let registry = registry();
        let plan = MenuPlan::build(&model(), &registry);
        let chord = |text| KeyChord::parse(text).unwrap();
        let mut bindings: Vec<_> = Keymap::defaults(&registry)
            .bindings()
            .iter()
            .filter(|binding| binding.command.0 != "search.find")
            .cloned()
            .collect();
        // A key sequence has no menu form.
        bindings.push(KeyBinding {
            command: CommandId("search.find"),
            sequence: vec![chord("Ctrl+K"), chord("Ctrl+F")],
        });
        // Save's first binding lands on ⌘Q, which Quit owns; its next one is used.
        bindings.insert(
            0,
            KeyBinding {
                command: CommandId("file.save"),
                sequence: vec![chord("Ctrl+Q")],
            },
        );
        let mut keymap = Keymap::default();
        keymap.replace(bindings, &registry).unwrap();
        let keys = plan.key_equivalents(&keymap);
        let key = |tag: isize| keys.get(&tag).cloned();
        assert_eq!(key(plan.tag(CommandId("search.find")).unwrap()), None);
        assert_eq!(
            key(plan.tag(CommandId("file.save")).unwrap()),
            Some(KeyEquivalent::new("s", ModifierMask::COMMAND))
        );
        // File > New is listed twice; only its first item shows ⌘N.
        let mut new_items = Vec::new();
        plan.walk(&mut |node| {
            if let PlanNode::Command(command) = node
                && command.command.0 == "file.new"
            {
                new_items.push(command.tag);
            }
        });
        assert_eq!(new_items.len(), 2);
        assert_eq!(key(new_items[0]), Some(KeyEquivalent::new("n", ModifierMask::COMMAND)));
        assert_eq!(key(new_items[1]), None);
    }

    #[test]
    fn projection_localizes_titles_and_reflects_state() {
        let registry = registry();
        let plan = MenuPlan::build(&model(), &registry);
        let mut context = CommandContext::default();
        context
            .states
            .insert(CommandId("file.save"), CommandState::disabled("Nothing to save"));
        context.states.insert(
            CommandId("search.find"),
            CommandState {
                checked: true,
                radio: true,
                label: Some("Find in R&D notes".into()),
                ..CommandState::default()
            },
        );
        let projection = plan.project(
            &registry,
            &context,
            &Keymap::defaults(&registry),
            |id, fallback| match id {
                "file.new" => "&Nouveau".into(),
                "menu.Edit" => "&Édition".into(),
                "macos.app.quit" => "Quitter Bareline".into(),
                _ => fallback.to_owned(),
            },
        );
        let item = |id| {
            let tag = plan.tag(CommandId(id)).unwrap();
            projection.items.iter().find(|item| item.tag == tag).unwrap().clone()
        };
        assert_eq!(item("file.new").title, "Nouveau");
        assert!(!item("file.save").enabled && item("file.new").enabled);
        // Live labels are shown as they are, ampersands included.
        let find = item("search.find");
        assert_eq!(find.title, "Find in R&D notes");
        assert!(find.checked && find.radio);
        assert_eq!(item("app.quit").title, "Quitter Bareline");
        assert_eq!(item("settings.open").title, "Settings\u{2026}");
        assert_eq!(
            item("edit.copy").key,
            Some(KeyEquivalent::new("c", ModifierMask::COMMAND))
        );
        assert_eq!(
            projection.menus,
            ["Bareline", "Services", "File", "Édition", "Search", "More", "Other"]
        );
        assert_eq!(projection.system, ["Hide Bareline", "Hide Others", "Show All"]);
        // One state per command item, in walk order.
        let mut tags = Vec::new();
        plan.walk(&mut |node| {
            if let PlanNode::Command(command) = node {
                tags.push(command.tag);
            }
        });
        assert_eq!(projection.items.iter().map(|item| item.tag).collect::<Vec<_>>(), tags);
    }

    #[test]
    fn mnemonic_markers_are_removed_from_titles() {
        assert_eq!(plain_title("&Save"), "Save");
        assert_eq!(plain_title("Do&n't Save"), "Don't Save");
        assert_eq!(plain_title("Rock && Roll"), "Rock & Roll");
        assert_eq!(plain_title("Trailing&"), "Trailing");
        assert_eq!(plain_title("Plain"), "Plain");
    }
}
