// SPDX-License-Identifier: MPL-2.0
//! Owned accessibility data. Providers cannot read documents or invoke UI code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
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
    TabList,
    Tab,
    Scrollbar,
    Tree,
    TreeItem,
    Editor,
    Status,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
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
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AccessibilityText {
    pub editor_id: u64,
    pub run_id: u64,
    pub start_byte: usize,
    pub value: String,
    pub character_lengths: Vec<u8>,
    pub selection: Option<(usize, usize)>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AccessibilitySnapshot {
    pub root: u64,
    pub focus: u64,
    pub nodes: Vec<AccessibilityNode>,
    pub text: Option<AccessibilityText>,
    pub text_context: Option<AccessibilityTextContext>,
    /// Visible grapheme boxes from actual shaped layouts, in physical client px.
    pub text_geometry: Vec<AccessibilityTextBox>,
    /// Additional independently owned editor text views. The legacy fields
    /// above remain the active single-view publication for non-split clients.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_views: Vec<AccessibilityTextView>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AccessibilityTextView {
    pub editor_id: u64,
    pub text: Option<AccessibilityText>,
    pub context: AccessibilityTextContext,
    pub geometry: Vec<AccessibilityTextBox>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AccessibilityTextBox {
    pub start: usize,
    pub end: usize,
    pub bounds: [f64; 4],
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AccessibilityTextContext {
    /// Identifies the immutable source used for this selection and viewport.
    pub source_identity: (u64, u64),
    pub selection: (usize, usize),
    /// Bounded selection rows; the primary pair above continues to describe the caret.
    pub selections: Vec<(usize, usize)>,
    /// Preedit is inserted at the caret in the provider's virtual text view;
    /// it never changes the committed document or its canonical offsets.
    pub composition: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessibilityAction {
    Focus(u64),
    Invoke(u64),
    SetValue {
        id: u64,
        value: String,
    },
    SetSelection {
        source_identity: (u64, u64),
        anchor: usize,
        caret: usize,
    },
    ModifySelection {
        source_identity: (u64, u64),
        start: usize,
        end: usize,
        add: bool,
    },
    /// Absolute UTF-8 text position; the UI owner requests a bounded viewport.
    ScrollToText {
        source_identity: (u64, u64),
        offset: usize,
    },
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
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Return at most `limit` bytes, aligned inward to complete UTF-8 code points.
    /// `limit` is never greater than 64 KiB. No full line/index scan is permitted.
    fn read(&self, start: usize, limit: usize) -> AccessibleRead;
}
impl AccessibilitySnapshot {
    pub fn text_view(
        &self,
        editor_id: u64,
    ) -> Option<(
        Option<&AccessibilityText>,
        &AccessibilityTextContext,
        &[AccessibilityTextBox],
    )> {
        if self.text.as_ref().is_some_and(|text| text.editor_id == editor_id) {
            return self
                .text_context
                .as_ref()
                .map(|context| (self.text.as_ref(), context, self.text_geometry.as_slice()));
        }
        if self.text.is_none() && self.text_views.is_empty() && editor_id == 2 {
            return self
                .text_context
                .as_ref()
                .map(|context| (None, context, self.text_geometry.as_slice()));
        }
        self.text_views
            .iter()
            .find(|view| view.editor_id == editor_id)
            .map(|view| (view.text.as_ref(), &view.context, view.geometry.as_slice()))
    }

    /// Validate before passing an external tree to the native provider.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text_geometry.len() > 4096
            || self.text_geometry.iter().any(|rect| {
                rect.start > rect.end
                    || rect.bounds.iter().any(|v| !v.is_finite())
                    || rect.bounds[2] < 0.0
                    || rect.bounds[3] < 0.0
            })
        {
            return Err("invalid bounded text geometry");
        }
        let ids: std::collections::BTreeSet<_> = self.nodes.iter().map(|n| n.id).collect();
        if ids.len() != self.nodes.len() || !ids.contains(&self.root) || !ids.contains(&self.focus) {
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
        let mut text_ids = std::collections::BTreeSet::new();
        let texts = self
            .text
            .iter()
            .chain(self.text_views.iter().filter_map(|view| view.text.as_ref()));
        for text in texts {
            if text.value.len() > 65536
                || !ids.contains(&text.editor_id)
                || ids.contains(&text.run_id)
                || !text_ids.insert(text.editor_id)
                || text.character_lengths.iter().map(|n| usize::from(*n)).sum::<usize>() != text.value.len()
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
            if text
                .selection
                .is_some_and(|(a, c)| a > text.character_lengths.len() || c > text.character_lengths.len())
            {
                return Err("selection outside text run");
            }
        }
        let view_ids: std::collections::BTreeSet<_> = self.text_views.iter().map(|view| view.editor_id).collect();
        if self.text_views.len() > 2
            || view_ids.len() != self.text_views.len()
            || self.text_views.iter().any(|view| {
                !ids.contains(&view.editor_id)
                    || view.geometry.len() > 4096
                    || view.geometry.iter().any(|rect| {
                        rect.start > rect.end
                            || rect.bounds.iter().any(|value| !value.is_finite())
                            || rect.bounds[2] < 0.0
                            || rect.bounds[3] < 0.0
                    })
                    || view.context.source_identity.0 != view.editor_id
                    || view.text.as_ref().is_some_and(|text| text.editor_id != view.editor_id)
            })
        {
            return Err("invalid editor text view");
        }
        Ok(())
    }
}
