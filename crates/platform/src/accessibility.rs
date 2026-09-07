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
    pub text_context: Option<AccessibilityTextContext>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessibilityTextContext {
    /// Identifies the immutable source used for this selection and viewport.
    pub source_identity: (u64, u64),
    pub selection: (usize, usize),
    /// Preedit is inserted at the selection in the provider's virtual text view;
    /// it never changes the committed document or its canonical offsets.
    pub composition: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessibilityAction {
    Focus(u64),
    Invoke(u64),
    SetValue { id: u64, value: String },
    SetSelection { source_identity: (u64, u64), anchor: usize, caret: usize },
    /// Absolute UTF-8 text position; the UI owner requests a bounded viewport.
    ScrollToText { source_identity: (u64, u64), offset: usize },
}

/// A read never waits for source I/O. Pending work is bounded by the source owner
/// and wakes the UI when ready. Positions address valid UTF-8 text, not raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessibleRead {
    Ready { start: usize, text: String },
    Pending,
    Unavailable,
}
pub trait AccessibilityTextSource: Send + Sync {
    /// Globally unique content state; ranges become unavailable on any revision
    /// or document switch. Undo restores content but still changes revision.
    fn identity(&self) -> (u64, u64);
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    /// Return at most `limit` bytes, aligned inward to complete UTF-8 code points.
    /// `limit` is never greater than 64 KiB. No full line/index scan is permitted.
    fn read(&self, start: usize, limit: usize) -> AccessibleRead;
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
