// SPDX-License-Identifier: MPL-2.0
//! Neutral semantic paint palette, supplied by the settings/theme owner per frame.
use bareline_renderer::Color;
/// Editor paint tokens remain separate from document layout and mutation state.
#[derive(Clone, Copy, Debug)]
pub struct EditorTheme {
    pub ui: UiTheme,
    pub current_line: Color,
    pub gutter: Color,
    pub keyword: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub operator: Color,
}
impl Default for EditorTheme {
    fn default() -> Self {
        Self {
            ui: UiTheme::default(),
            current_line: Color(0x262B31),
            gutter: Color(0x9AA3AD),
            keyword: Color(0xC79BFF),
            string: Color(0xA5D6A7),
            number: Color(0xF5B76B),
            comment: Color(0x9AA3AD),
            operator: Color(0xE6E8EA),
        }
    }
}
impl EditorTheme {
    pub fn from_tokens(mut lookup: impl FnMut(&str) -> Option<(u32, u8)>) -> Option<Self> {
        let ui = UiTheme::from_tokens(&mut lookup)?;
        let mut color = |key: &str| {
            let (rgb, alpha) = lookup(key)?;
            let mut result = 0;
            for shift in [16, 8, 0] {
                result |= ((((rgb >> shift) & 255) * alpha as u32
                    + ((ui.editor.0 >> shift) & 255) * (255 - alpha as u32)
                    + 127)
                    / 255)
                    << shift;
            }
            Some(Color(result))
        };
        Some(Self {
            ui,
            current_line: color("surface.currentLine")?,
            gutter: color("text.gutter")?,
            keyword: color("syntax.keyword")?,
            string: color("syntax.string")?,
            number: color("syntax.number")?,
            comment: color("syntax.comment")?,
            operator: color("syntax.operator")?,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct UiTheme {
    pub editor: Color,
    pub chrome: Color,
    pub elevated: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub interactive: Color,
    pub focus: Color,
    pub selection: Color,
    pub caret: Color,
}
impl Default for UiTheme {
    fn default() -> Self {
        Self {
            editor: Color(0x1F2328),
            chrome: Color(0x181B1F),
            elevated: Color(0x262B31),
            text: Color(0xE6E8EA),
            muted: Color(0x9AA3AD),
            border: Color(0x343A42),
            interactive: Color(0x9AA3AD),
            focus: Color(0x2ED3C4),
            selection: Color(0x22524A),
            caret: Color(0x2ED3C4),
        }
    }
}
impl UiTheme {
    /// RGBA token lookup avoids a dependency from reusable controls to settings storage.
    pub fn from_tokens(mut lookup: impl FnMut(&str) -> Option<(u32, u8)>) -> Option<Self> {
        let editor = lookup("surface.editor")?.0;
        let mut color = |key: &str| {
            let (rgb, alpha) = lookup(key)?;
            let mut result = 0;
            for shift in [16, 8, 0] {
                let a = (rgb >> shift) & 255;
                let b = (editor >> shift) & 255;
                result |= ((a * alpha as u32 + b * (255 - alpha as u32) + 127) / 255) << shift;
            }
            Some(Color(result))
        };
        Some(Self {
            editor: Color(editor),
            chrome: color("surface.chrome")?,
            elevated: color("surface.elevated")?,
            text: color("text")?,
            muted: color("text.muted")?,
            border: color("border")?,
            interactive: color("border.interactive")?,
            focus: color("focus.ring")?,
            selection: color("selection")?,
            caret: color("caret")?,
        })
    }
    pub fn widgets(self) -> crate::widgets::Theme {
        crate::widgets::Theme {
            surface: self.elevated,
            text: self.text,
            muted: self.muted,
            selection: self.selection,
            border: self.interactive,
            focus: self.focus,
        }
    }
}
