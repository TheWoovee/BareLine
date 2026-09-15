// SPDX-License-Identifier: MPL-2.0
use crate::{Action, CommandId, CommandRegistry, Keymap};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandState {
    pub enabled: bool,
    pub checked: bool,
    /// Renders the check mark as an exclusive radio dot (e.g. the active window).
    pub radio: bool,
    /// Contextual commands that do not apply right now are removed from menus
    /// entirely instead of shown greyed out.
    pub hidden: bool,
    pub label: Option<String>,
    pub disabled_reason: Option<String>,
}
impl Default for CommandState {
    fn default() -> Self {
        Self {
            enabled: true,
            checked: false,
            radio: false,
            hidden: false,
            label: None,
            disabled_reason: None,
        }
    }
}
impl CommandState {
    pub fn disabled(reason: impl Into<String>) -> Self {
        Self {
            enabled: false,
            disabled_reason: Some(reason.into()),
            ..Self::default()
        }
    }
    /// A contextual command that simply does not apply in the current state. It
    /// stays dispatchable-in-theory but is hidden from menus rather than shown
    /// as a greyed-out row that only clutters the list.
    pub fn not_applicable(reason: impl Into<String>) -> Self {
        Self {
            enabled: false,
            hidden: true,
            disabled_reason: Some(reason.into()),
            ..Self::default()
        }
    }
}
/// Immutable snapshot supplied by the composition root at dispatch time.
#[derive(Default, Clone, Debug)]
pub struct CommandContext {
    pub states: BTreeMap<CommandId, CommandState>,
    pub show_internal: bool,
}
#[derive(Default, Clone, Debug)]
pub struct CommandPresentation {
    pub menu_path: String,
    pub keywords: Vec<String>,
    pub accessible_name: Option<String>,
    pub internal: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    Unknown(CommandId),
    Disabled(String),
}
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub dynamic: Option<crate::DynamicCommandIdentity>,
    pub id: CommandId,
    pub title: String,
    pub menu_path: String,
    pub shortcut: String,
    pub accessible_name: String,
    pub state: CommandState,
    pub score: usize,
}
impl CommandRegistry {
    pub fn lookup(&self, id: &str) -> Option<CommandId> {
        self.entries.keys().find(|key| key.0 == id).copied()
    }
    pub fn presentation(&self, id: CommandId) -> Option<&CommandPresentation> {
        self.presentations.get(&id)
    }
    pub fn set_presentation(&mut self, id: CommandId, presentation: CommandPresentation) -> Result<(), CommandId> {
        if !self.entries.contains_key(&id) {
            return Err(id);
        }
        self.presentations.insert(id, presentation);
        Ok(())
    }
    pub fn state(&self, id: CommandId, context: &CommandContext) -> Option<CommandState> {
        self.entries
            .get(&id)
            .map(|_| context.states.get(&id).cloned().unwrap_or_default())
    }
    pub fn dispatch_in(&self, id: CommandId, context: &CommandContext) -> Result<Action, DispatchError> {
        let state = self.state(id, context).ok_or(DispatchError::Unknown(id))?;
        if !state.enabled {
            return Err(DispatchError::Disabled(
                state
                    .disabled_reason
                    .unwrap_or_else(|| "Unavailable in the current context".into()),
            ));
        }
        self.dispatch(id).ok_or(DispatchError::Unknown(id))
    }
    pub fn palette(&self, query: &str, context: &CommandContext, keymap: &Keymap, limit: usize) -> Vec<PaletteEntry> {
        let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut matches = Vec::new();
        for spec in self.entries() {
            let metadata = self.presentation(spec.id).cloned().unwrap_or_default();
            if metadata.internal && !context.show_internal {
                continue;
            }
            let state = self.state(spec.id, context).unwrap();
            let title = state.label.clone().unwrap_or_else(|| spec.title.into());
            let path = if metadata.menu_path.is_empty() {
                spec.category.into()
            } else {
                metadata.menu_path
            };
            let shortcut = keymap.shortcut_label(spec.id);
            let title_haystack = title.to_lowercase();
            let aux_haystack = format!(
                "{} {} {} {}",
                spec.category,
                path,
                metadata.keywords.join(" "),
                shortcut
            )
            .to_lowercase();
            let Some(score) = query_score(&terms, &title_haystack, &aux_haystack) else {
                continue;
            };
            matches.push(PaletteEntry {
                dynamic: None,
                id: spec.id,
                accessible_name: metadata.accessible_name.unwrap_or_else(|| title.clone()),
                title,
                menu_path: path,
                shortcut,
                state,
                score,
            });
        }
        for record in self.contributions.entries() {
            let title_haystack = record.title.to_lowercase();
            let aux_haystack = format!("{} {}", record.identity.owner, record.identity.id).to_lowercase();
            let Some(score) = query_score(&terms, &title_haystack, &aux_haystack) else {
                continue;
            };
            matches.push(PaletteEntry {
                dynamic: Some(record.identity.clone()),
                id: CommandId("internal.dynamic.invoke"),
                title: record.title.clone(),
                accessible_name: record.title.clone(),
                menu_path: format!("Extensions > {}", record.identity.owner),
                shortcut: String::new(),
                score,
                state: CommandState {
                    enabled: record.enabled,
                    disabled_reason: (!record.enabled).then(|| {
                        record
                            .disabled_reason
                            .clone()
                            .unwrap_or_else(|| "Extension command unavailable".into())
                    }),
                    ..Default::default()
                },
            });
        }
        matches.sort_by(|a, b| {
            // Highest score first; then (recency is not tracked at this layer)
            // category, then the closest match — a shorter title has less
            // unmatched text — then alphabetical and the stable ID.
            b.score
                .cmp(&a.score)
                .then_with(|| a.menu_path.cmp(&b.menu_path))
                .then_with(|| a.title.chars().count().cmp(&b.title.chars().count()))
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });
        matches.truncate(limit);
        matches
    }
}

/// Sum each query term's best match against the title and its auxiliary text.
/// Returns `None` when any term matches nowhere; an empty query scores every
/// command at zero so the palette can list them all.
fn query_score(terms: &[String], title: &str, aux: &str) -> Option<usize> {
    let mut score = 0usize;
    for term in terms {
        score += term_score(term, title, aux)?;
    }
    Some(score)
}

/// A single term's best tier: an exact title prefix (100), a title word-start
/// prefix (80) or a contiguous title substring (60) always beat a scattered
/// subsequence (20 minus a gap penalty). Auxiliary text — category, menu path,
/// keywords, shortcut — matches at a lower weight so real titles rank first.
fn term_score(term: &str, title: &str, aux: &str) -> Option<usize> {
    if title.starts_with(term) {
        Some(100)
    } else if word_start_prefix(title, term) {
        Some(80)
    } else if title.contains(term) {
        Some(60)
    } else if let Some(score) = subsequence_score(term, title) {
        Some(score.max(1))
    } else if aux.contains(term) {
        Some(50)
    } else {
        subsequence_score(term, aux).map(|score| (score / 2).max(1))
    }
}

/// True when any word inside `haystack` (split on non-alphanumeric boundaries)
/// begins with `term`, e.g. the "As" in "Save As".
fn word_start_prefix(haystack: &str, term: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(term))
}

/// Score a scattered subsequence match: base 20 reduced by the number of
/// characters skipped over. `None` if `needle` is not a subsequence at all.
fn subsequence_score(needle: &str, haystack: &str) -> Option<usize> {
    let mut chars = haystack.chars();
    let mut gaps = 0usize;
    for wanted in needle.chars() {
        let mut matched = false;
        for actual in chars.by_ref() {
            if actual == wanted {
                matched = true;
                break;
            }
            gaps += 1;
        }
        if !matched {
            return None;
        }
    }
    Some(20usize.saturating_sub(gaps))
}

pub const MENU_TAXONOMY: [&str; 12] = [
    "File", "Edit", "Search", "View", "Encoding", "Language", "Settings", "Macro", "Run", "Tools", "Window", "Help",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Command(CommandId),
    Separator,
    Submenu { title: String, items: Vec<MenuItem> },
}
/// A hand-authored menu tree, referencing commands by their stable string ID so
/// the taxonomy can be written declaratively (see `bareline_app::menus`).
#[derive(Clone, Copy, Debug)]
pub enum MenuTemplate {
    Command(&'static str),
    Separator,
    Submenu(&'static str, &'static [MenuTemplate]),
}
/// Folds `Submenu { title: T, [Command(id)] }` into `Command(id)` when the lone
/// command's title equals `T`. Recurses so nested submenus collapse too.
fn collapse_single_item_submenus(items: &mut Vec<MenuItem>, registry: &CommandRegistry) {
    for item in items.iter_mut() {
        if let MenuItem::Submenu { items: children, .. } = item {
            collapse_single_item_submenus(children, registry);
        }
    }
    for item in items.iter_mut() {
        let collapsed = if let MenuItem::Submenu { title, items: children } = item {
            if let [MenuItem::Command(id)] = children.as_slice() {
                (registry.entries().find(|spec| spec.id == *id).map(|spec| spec.title) == Some(title.as_str()))
                    .then_some(*id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(id) = collapsed {
            *item = MenuItem::Command(id);
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuModel {
    pub items: Vec<MenuItem>,
}
impl MenuModel {
    pub fn from_registry(registry: &CommandRegistry) -> Self {
        let mut model = Self {
            items: MENU_TAXONOMY
                .iter()
                .map(|category| MenuItem::Submenu {
                    title: (*category).into(),
                    items: Vec::new(),
                })
                .collect(),
        };
        fn insert(items: &mut Vec<MenuItem>, path: &[&str], id: CommandId) {
            let Some((title, rest)) = path.split_first() else {
                items.push(MenuItem::Command(id));
                return;
            };
            let index = items
                .iter()
                .position(|item| matches!(item, MenuItem::Submenu { title: current, .. } if current == title));
            let index = index.unwrap_or_else(|| {
                items.push(MenuItem::Submenu {
                    title: (*title).into(),
                    items: Vec::new(),
                });
                items.len() - 1
            });
            if let MenuItem::Submenu { items, .. } = &mut items[index] {
                insert(items, rest, id);
            }
        }
        for spec in registry.entries() {
            let metadata = registry.presentation(spec.id);
            if metadata.is_some_and(|value| value.internal) {
                continue;
            }
            let path = metadata
                .map(|value| value.menu_path.as_str())
                .filter(|path| !path.is_empty())
                .unwrap_or(spec.category);
            let parts: Vec<_> = path
                .split(['>', '›'])
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect();
            insert(&mut model.items, &parts, spec.id);
        }
        // A submenu that ends up holding a single command whose title matches the
        // submenu label is pure friction ("Find in Folder… ▸ Find in Folder…"):
        // fold it back into a plain command. Top-level taxonomy menus are left
        // intact so the menu bar always shows the twelve categories.
        for top in &mut model.items {
            if let MenuItem::Submenu { items, .. } = top {
                collapse_single_item_submenus(items, registry);
            }
        }
        model
    }
    /// Build the menu from a hand-authored taxonomy tree rather than from raw
    /// registration order. Command IDs missing from the registry are skipped
    /// (optional features), and every non-internal command the tree does not
    /// place explicitly is collected into a Tools ▸ Other submenu so nothing is
    /// ever silently unreachable.
    pub fn curated(registry: &CommandRegistry, tree: &[MenuTemplate]) -> Self {
        let mut placed = std::collections::BTreeSet::new();
        fn build(
            templates: &[MenuTemplate],
            registry: &CommandRegistry,
            placed: &mut std::collections::BTreeSet<CommandId>,
        ) -> Vec<MenuItem> {
            let mut items = Vec::new();
            for template in templates {
                match *template {
                    MenuTemplate::Separator => {
                        // Never open or double a separator; drop leading/dup ones.
                        if matches!(items.last(), None | Some(MenuItem::Separator)) {
                            continue;
                        }
                        items.push(MenuItem::Separator);
                    }
                    MenuTemplate::Command(id) => {
                        if let Some(id) = registry.lookup(id) {
                            // Mark it placed so it is not also auto-routed, but keep
                            // internal commands out of the visible tree.
                            placed.insert(id);
                            if !registry.presentation(id).is_some_and(|meta| meta.internal) {
                                items.push(MenuItem::Command(id));
                            }
                        }
                    }
                    MenuTemplate::Submenu(title, children) => {
                        let children = build(children, registry, placed);
                        if children.iter().any(|item| !matches!(item, MenuItem::Separator)) {
                            items.push(MenuItem::Submenu {
                                title: title.into(),
                                items: children,
                            });
                        }
                    }
                }
            }
            while matches!(items.last(), Some(MenuItem::Separator)) {
                items.pop();
            }
            items
        }
        let mut model = Self {
            items: build(tree, registry, &mut placed),
        };
        // Ensure Tools exists as the home for the Other catch-all.
        if !model
            .items
            .iter()
            .any(|item| matches!(item, MenuItem::Submenu { title, .. } if title == "Tools"))
        {
            model.items.push(MenuItem::Submenu {
                title: "Tools".into(),
                items: Vec::new(),
            });
        }
        let tops: std::collections::BTreeSet<String> = model
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Submenu { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect();
        fn insert(items: &mut Vec<MenuItem>, path: &[&str], id: CommandId) {
            let Some((title, rest)) = path.split_first() else {
                items.push(MenuItem::Command(id));
                return;
            };
            let index = items
                .iter()
                .position(|item| matches!(item, MenuItem::Submenu { title: current, .. } if current == title))
                .unwrap_or_else(|| {
                    items.push(MenuItem::Submenu {
                        title: (*title).into(),
                        items: Vec::new(),
                    });
                    items.len() - 1
                });
            if let MenuItem::Submenu { items, .. } = &mut items[index] {
                insert(items, rest, id);
            }
        }
        // Anything the tree did not place explicitly is auto-routed by its
        // declared menu path so new or forgotten commands still surface in a
        // sensible place. Only paths that begin with one of the taxonomy menus
        // are honored; legacy categories (Utilities, Extensions, Workspace, …)
        // must not resurrect a stray top-level menu, so they fall to Tools ▸ Other.
        for spec in registry.entries() {
            if placed.contains(&spec.id) || registry.presentation(spec.id).is_some_and(|meta| meta.internal) {
                continue;
            }
            let path = registry
                .presentation(spec.id)
                .map(|meta| meta.menu_path.as_str())
                .filter(|path| !path.is_empty())
                .unwrap_or(spec.category);
            let parts: Vec<&str> = path
                .split(['>', '›'])
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect();
            if parts.first().is_some_and(|first| tops.contains(*first)) {
                insert(&mut model.items, &parts, spec.id);
            } else {
                insert(&mut model.items, &["Tools", "Other"], spec.id);
            }
        }
        for top in &mut model.items {
            if let MenuItem::Submenu { items, .. } = top {
                collapse_single_item_submenus(items, registry);
            }
        }
        model
    }
    /// The subset of the menu that should actually render right now: internal
    /// commands and contextual commands hidden by their `CommandState` are
    /// dropped, submenus left empty are removed, and separators are tidied so
    /// none lead, trail, or double up. Used by the native layer to decide when
    /// the on-screen menu structure must be rebuilt.
    pub fn visible(&self, registry: &CommandRegistry, context: &CommandContext) -> Self {
        fn filter(items: &[MenuItem], registry: &CommandRegistry, context: &CommandContext) -> Vec<MenuItem> {
            let mut out: Vec<MenuItem> = Vec::new();
            for item in items {
                match item {
                    MenuItem::Command(id) => {
                        let internal = registry.presentation(*id).is_some_and(|meta| meta.internal);
                        let hidden = context.states.get(id).is_some_and(|state| state.hidden);
                        if !internal && !hidden {
                            out.push(MenuItem::Command(*id));
                        }
                    }
                    MenuItem::Submenu { title, items } => {
                        let children = filter(items, registry, context);
                        if children.iter().any(|item| !matches!(item, MenuItem::Separator)) {
                            out.push(MenuItem::Submenu {
                                title: title.clone(),
                                items: children,
                            });
                        }
                    }
                    MenuItem::Separator => {
                        if !matches!(out.last(), None | Some(MenuItem::Separator)) {
                            out.push(MenuItem::Separator);
                        }
                    }
                }
            }
            while matches!(out.last(), Some(MenuItem::Separator)) {
                out.pop();
            }
            out
        }
        Self {
            items: filter(&self.items, registry, context),
        }
    }
    /// Command IDs in menu order — the fingerprint the native layer compares to
    /// know whether the visible structure changed since the last build.
    pub fn command_order(&self) -> Vec<CommandId> {
        fn walk(items: &[MenuItem], out: &mut Vec<CommandId>) {
            for item in items {
                match item {
                    MenuItem::Command(id) => out.push(*id),
                    MenuItem::Submenu { items, .. } => walk(items, out),
                    MenuItem::Separator => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }
    /// Context owners supply applicable IDs; absent feature commands are never fabricated.
    pub fn for_context(commands: &[CommandId], registry: &CommandRegistry) -> Result<Self, CommandId> {
        for id in commands {
            if registry.dispatch(*id).is_none() {
                return Err(*id);
            }
        }
        Ok(Self {
            items: commands.iter().copied().map(MenuItem::Command).collect(),
        })
    }
    pub fn validate(&self, registry: &CommandRegistry) -> Vec<CommandId> {
        fn visit(items: &[MenuItem], registry: &CommandRegistry, missing: &mut Vec<CommandId>) {
            for item in items {
                match item {
                    MenuItem::Command(id) if registry.dispatch(*id).is_none() => missing.push(*id),
                    MenuItem::Submenu { items, .. } => visit(items, registry, missing),
                    _ => {}
                }
            }
        }
        let mut missing = Vec::new();
        visit(&self.items, registry, &mut missing);
        missing.sort();
        missing.dedup();
        missing
    }
}
#[derive(Clone, Debug, Default)]
pub struct ToolbarModel {
    pub visible: bool,
    pub commands: Vec<CommandId>,
}
impl ToolbarModel {
    pub fn set_commands(&mut self, commands: Vec<CommandId>, registry: &CommandRegistry) -> Result<(), CommandId> {
        if let Some(id) = commands.iter().find(|id| registry.dispatch(**id).is_none()) {
            return Err(*id);
        }
        self.commands = commands;
        Ok(())
    }
}
/// Escape only dismisses the top transient layer, returning its invoking focus token.
#[derive(Clone, Debug, Default)]
pub struct FocusStack {
    layers: Vec<FocusLayer>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusLayer {
    pub name: String,
    pub return_focus: String,
    pub owned_commands: Vec<CommandId>,
}
impl FocusStack {
    pub fn push(&mut self, layer: FocusLayer) {
        self.layers.push(layer);
    }
    pub fn escape(&mut self) -> Option<String> {
        self.layers.pop().map(|layer| layer.return_focus)
    }
    pub fn owner(&self, command: CommandId) -> Option<&FocusLayer> {
        self.layers
            .iter()
            .rev()
            .find(|layer| layer.owned_commands.contains(&command))
    }
    pub fn active(&self) -> Option<&FocusLayer> {
        self.layers.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    #[test]
    fn disabled_command_discoverable_but_cannot_dispatch() {
        let registry = shell_commands();
        let keymap = Keymap::defaults(&registry);
        let mut context = CommandContext::default();
        context
            .states
            .insert(CommandId("file.save"), CommandState::disabled("Document is read-only"));
        let result = registry.palette("save", &context, &keymap, 20);
        assert!(
            result
                .iter()
                .any(|entry| entry.id == CommandId("file.save") && !entry.state.enabled)
        );
        assert_eq!(
            registry.dispatch_in(CommandId("file.save"), &context),
            Err(DispatchError::Disabled("Document is read-only".into()))
        );
        assert!(
            registry
                .palette("locate", &context, &keymap, 20)
                .iter()
                .any(|entry| entry.id == CommandId("search.find"))
        );
        assert!(
            registry
                .palette("cmdplt", &context, &keymap, 20)
                .iter()
                .any(|entry| entry.id == CommandId("view.command_palette"))
        );
        assert!(
            registry
                .palette("no such command xyz", &context, &keymap, 20)
                .is_empty()
        );
    }
    #[test]
    fn projections_and_customizations_validate_targets() {
        let mut registry = shell_commands();
        let menu = MenuModel::from_registry(&registry);
        assert_eq!(menu.items.len(), 12);
        assert!(menu.validate(&registry).is_empty());
        let mut toolbar = ToolbarModel::default();
        assert!(!toolbar.visible);
        toolbar.set_commands(vec![CommandId("file.save")], &registry).unwrap();
        assert!(toolbar.set_commands(vec![CommandId("unknown")], &registry).is_err());
        assert_eq!(toolbar.commands, vec![CommandId("file.save")]);
        registry
            .set_presentation(
                CommandId("file.save"),
                CommandPresentation {
                    menu_path: "File > Write".into(),
                    accessible_name: Some("Write document".into()),
                    internal: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let keymap = Keymap::defaults(&registry);
        assert!(
            registry
                .palette("Write", &CommandContext::default(), &keymap, 100)
                .iter()
                .all(|entry| entry.id != CommandId("file.save"))
        );
        let entries = registry.palette(
            "File Write",
            &CommandContext {
                show_internal: true,
                ..Default::default()
            },
            &keymap,
            100,
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.id == CommandId("file.save"))
                .unwrap()
                .accessible_name,
            "Write document"
        );
    }
    fn reachable(items: &[MenuItem], out: &mut std::collections::BTreeSet<CommandId>) {
        for item in items {
            match item {
                MenuItem::Command(id) => {
                    out.insert(*id);
                }
                MenuItem::Submenu { items, .. } => reachable(items, out),
                MenuItem::Separator => {}
            }
        }
    }
    #[test]
    fn no_single_item_submenu_survives_from_registry() {
        let mut registry = shell_commands();
        // A registration that names the menu item as its own submenu segment —
        // exactly the pattern that produced "Find in Folder… ▸ Find in Folder…".
        registry
            .register(CommandSpec {
                id: CommandId("search.folder"),
                title: "Find in Folder…",
                category: "Search",
                shortcut: "",
                action: Action::FindNext,
            })
            .unwrap();
        registry
            .set_presentation(
                CommandId("search.folder"),
                CommandPresentation {
                    menu_path: "Search > Find in Folder…".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let model = MenuModel::from_registry(&registry);
        fn assert_no_singletons(items: &[MenuItem], registry: &CommandRegistry) {
            for item in items {
                if let MenuItem::Submenu { title, items } = item {
                    if items.len() == 1
                        && let MenuItem::Command(id) = items[0]
                    {
                        let inner = registry.entries().find(|s| s.id == id).map(|s| s.title);
                        assert_ne!(
                            inner,
                            Some(title.as_str()),
                            "single-item submenu {title:?} was not collapsed"
                        );
                    }
                    assert_no_singletons(items, registry);
                }
            }
        }
        assert_no_singletons(&model.items, &registry);
        let search = model
            .items
            .iter()
            .find_map(|i| match i {
                MenuItem::Submenu { title, items } if title == "Search" => Some(items),
                _ => None,
            })
            .unwrap();
        assert!(search.contains(&MenuItem::Command(CommandId("search.folder"))));
    }
    #[test]
    fn curated_routes_every_non_internal_command_and_hides_internal() {
        let registry = shell_commands();
        // Route only a couple commands explicitly; everything else must land in
        // Tools ▸ Other rather than vanish.
        static TREE: &[MenuTemplate] = &[
            MenuTemplate::Submenu(
                "File",
                &[MenuTemplate::Command("file.new"), MenuTemplate::Command("file.open")],
            ),
            MenuTemplate::Submenu("Tools", &[]),
        ];
        let model = MenuModel::curated(&registry, TREE);
        let mut placed = std::collections::BTreeSet::new();
        reachable(&model.items, &mut placed);
        for spec in registry.entries() {
            let internal = registry.presentation(spec.id).is_some_and(|meta| meta.internal);
            if internal {
                assert!(
                    !placed.contains(&spec.id),
                    "internal command {} leaked into the menus",
                    spec.id.0
                );
            } else {
                assert!(
                    placed.contains(&spec.id),
                    "non-internal command {} is unreachable",
                    spec.id.0
                );
            }
        }
    }
    #[test]
    fn nested_escape_restores_only_one_invoker() {
        let mut stack = FocusStack::default();
        stack.push(FocusLayer {
            name: "palette".into(),
            return_focus: "editor".into(),
            owned_commands: vec![CommandId("file.open")],
        });
        stack.push(FocusLayer {
            name: "popup".into(),
            return_focus: "palette.input".into(),
            owned_commands: vec![],
        });
        assert_eq!(stack.escape().as_deref(), Some("palette.input"));
        assert_eq!(stack.active().unwrap().name, "palette");
        assert_eq!(stack.escape().as_deref(), Some("editor"));
        assert_eq!(stack.escape(), None);
    }
    #[test]
    fn scoring_prefers_prefix_titles_and_orders_save_variants() {
        let mut registry = CommandRegistry::default();
        for (id, title) in [
            ("app.settings", "Settings"),
            ("hash.sha1", "SHA-1"),
            ("hash.sha256", "SHA-256"),
            ("hash.sha512", "SHA-512"),
            ("file.save", "Save"),
            ("file.save_as", "Save As"),
            ("file.save_all", "Save All"),
            ("file.save_copy", "Save Copy"),
        ] {
            registry
                .register(CommandSpec {
                    id: CommandId(id),
                    title,
                    category: "File",
                    shortcut: "",
                    action: Action::About,
                })
                .unwrap();
        }
        let keymap = Keymap::defaults(&registry);
        let context = CommandContext::default();
        // `sett` is an exact prefix of Settings only; the hash commands do not
        // match at all, so Settings is first (and the only) result.
        let sett = registry.palette("sett", &context, &keymap, 12);
        assert_eq!(sett.first().map(|entry| entry.id), Some(CommandId("app.settings")));
        // `sav` prefixes every Save command; equal scores fall back to the
        // closest (shortest) title, so the variants come out shortest-first.
        let sav: Vec<_> = registry
            .palette("sav", &context, &keymap, 12)
            .into_iter()
            .map(|entry| entry.title)
            .collect();
        assert_eq!(sav, vec!["Save", "Save As", "Save All", "Save Copy"]);
    }
}
