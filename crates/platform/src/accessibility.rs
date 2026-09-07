// SPDX-License-Identifier: MPL-2.0
//! Owned accessibility data. Providers cannot read documents or invoke UI code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessibilityRole {
    Window,
    Group,
    Button,
    Checkbox,
    Radio,
    Combo,
    List,
    ListItem,
    TextField,
    Slider,
    SpinButton,
    Separator,
    Tooltip,
    Alert,
    Tab,
    Scrollbar,
    Tree,
    TreeItem,
    Editor,
    Status,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityNode {
    pub id: u64,
    pub parent: u64,
    pub role: AccessibilityRole,
    pub name: String,
    pub value: Option<String>,
    pub bounds: [f64; 4],
    pub disabled: bool,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub focusable: bool,
    pub invokable: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityText {
    pub editor_id: u64,
    pub run_id: u64,
    pub start_byte: usize,
    pub value: String,
    pub character_lengths: Vec<u8>,
    pub selection: Option<(usize, usize)>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilitySnapshot {
    pub root: u64,
    pub focus: u64,
    pub nodes: Vec<AccessibilityNode>,
    pub text: Option<AccessibilityText>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessibilityAction {
    Focus(u64),
    Invoke(u64),
    SetValue { id: u64, value: String },
    SetSelection { anchor: usize, caret: usize },
}
impl AccessibilitySnapshot {
    /// Validate before passing an external tree to the native provider.
    pub fn validate(&self) -> Result<(), &'static str> {
        let ids: std::collections::BTreeSet<_> = self.nodes.iter().map(|n| n.id).collect();
        if ids.len() != self.nodes.len() || !ids.contains(&self.root) || !ids.contains(&self.focus)
        {
            return Err("duplicate node or missing root/focus");
        }
        for node in &self.nodes {
            let mut parent = node.id;
            for step in 0..=self.nodes.len() {
                if parent == self.root {
                    break;
                }
                if step == self.nodes.len() {
                    return Err("cyclic semantic tree");
                }
                parent = self
                    .nodes
                    .iter()
                    .find(|n| n.id == parent)
                    .ok_or("missing semantic parent")?
                    .parent;
            }
        }
        if let Some(text) = &self.text {
            if text.value.len() > 65536
                || !ids.contains(&text.editor_id)
                || ids.contains(&text.run_id)
                || text
                    .character_lengths
                    .iter()
                    .map(|n| usize::from(*n))
                    .sum::<usize>()
                    != text.value.len()
            {
                return Err("invalid bounded text run");
            }
            let mut offset = 0;
            for length in &text.character_lengths {
                offset += usize::from(*length);
                if *length == 0 || !text.value.is_char_boundary(offset) {
                    return Err("invalid text boundary");
                }
            }
            if text.selection.is_some_and(|(a, c)| {
                a > text.character_lengths.len() || c > text.character_lengths.len()
            }) {
                return Err("selection outside text run");
            }
        }
        Ok(())
    }
}
