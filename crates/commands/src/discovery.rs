// SPDX-License-Identifier: MPL-2.0
use crate::{Action, CommandId, CommandRegistry, Keymap};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandState {
    pub enabled: bool,
    pub checked: bool,
    pub label: Option<String>,
    pub disabled_reason: Option<String>,
}
impl Default for CommandState {
    fn default() -> Self {
        Self {
            enabled: true,
            checked: false,
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
    pub fn set_presentation(
        &mut self,
        id: CommandId,
        presentation: CommandPresentation,
    ) -> Result<(), CommandId> {
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
    pub fn dispatch_in(
        &self,
        id: CommandId,
        context: &CommandContext,
    ) -> Result<Action, DispatchError> {
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
    pub fn palette(
        &self,
        query: &str,
        context: &CommandContext,
        keymap: &Keymap,
        limit: usize,
    ) -> Vec<PaletteEntry> {
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
            let haystack = format!(
                "{} {} {} {} {}",
                title,
                spec.category,
                path,
                metadata.keywords.join(" "),
                shortcut
            )
            .to_lowercase();
            let mut score = 0;
            let mut found = true;
            for term in &terms {
                if let Some(position) = haystack.find(term) {
                    score += 1000usize.saturating_sub(position);
                } else if is_subsequence(term, &haystack) {
                    score += 1;
                } else {
                    found = false;
                    break;
                }
            }
            if found {
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
        }
        for record in self.contributions.entries() {
            let haystack = format!(
                "{} {} {}",
                record.title, record.identity.owner, record.identity.id
            )
            .to_lowercase();
            if !terms
                .iter()
                .all(|term| haystack.contains(term) || is_subsequence(term, &haystack))
            {
                continue;
            }
            let score = terms
                .iter()
                .map(|term| {
                    haystack
                        .find(term)
                        .map_or(1, |position| 1000usize.saturating_sub(position))
                })
                .sum();
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
            b.score
                .cmp(&a.score)
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });
        matches.truncate(limit);
        matches
    }
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|wanted| chars.by_ref().any(|actual| actual == wanted))
}

pub const MENU_TAXONOMY: [&str; 12] = [
    "File", "Edit", "Search", "View", "Encoding", "Language", "Settings", "Macro", "Run", "Tools",
    "Window", "Help",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Command(CommandId),
    Separator,
    Submenu { title: String, items: Vec<MenuItem> },
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
            let index = items.iter().position(
                |item| matches!(item, MenuItem::Submenu { title: current, .. } if current == title),
            );
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
        model
    }
    /// Context owners supply applicable IDs; absent feature commands are never fabricated.
    pub fn for_context(
        commands: &[CommandId],
        registry: &CommandRegistry,
    ) -> Result<Self, CommandId> {
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
    pub fn set_commands(
        &mut self,
        commands: Vec<CommandId>,
        registry: &CommandRegistry,
    ) -> Result<(), CommandId> {
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
        context.states.insert(
            CommandId("file.save"),
            CommandState::disabled("Document is read-only"),
        );
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
        toolbar
            .set_commands(vec![CommandId("file.save")], &registry)
            .unwrap();
        assert!(
            toolbar
                .set_commands(vec![CommandId("unknown")], &registry)
                .is_err()
        );
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
}
