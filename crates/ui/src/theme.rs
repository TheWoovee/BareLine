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
    pub marks: [Color; 5],
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
            marks: [
                Color(0x1E3A2F),
                Color(0x3A3420),
                Color(0x23303F),
                Color(0x3D2426),
                Color(0x342740),
            ],
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
            marks: [
                color("mark.style1")?,
                color("mark.style2")?,
                color("mark.style3")?,
                color("mark.style4")?,
                color("mark.style5")?,
            ],
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
    /// Widget palette for dock/list panels, where the selection band and the
    /// separators both use the border colour. This is the single derivation for
    /// that palette so panels no longer each build it inline (ARCH-18).
    pub fn panel(self) -> crate::widgets::Theme {
        crate::widgets::Theme {
            surface: self.elevated,
            text: self.text,
            muted: self.muted,
            selection: self.border,
            border: self.border,
            focus: self.focus,
        }
    }
    /// Blend `over` toward this theme's editor background by `alpha`/255, so a
    /// fixed severity hue lands on colours that suit the current background in
    /// both the light and the dark theme.
    fn tint(self, over: Color, alpha: u32) -> Color {
        let mut result = 0;
        for shift in [16, 8, 0] {
            let a = (over.0 >> shift) & 255;
            let b = (self.editor.0 >> shift) & 255;
            result |= ((a * alpha + b * (255 - alpha) + 127) / 255) << shift;
        }
        Color(result)
    }
    /// Neutral floating-surface palette (toast, banner, small overlay) taken
    /// straight from the theme so every custom surface follows it (UX-55).
    pub fn overlay(self) -> OverlayPalette {
        OverlayPalette {
            surface: self.elevated,
            border: self.border,
            text: self.text,
            muted: self.muted,
            accent: self.focus,
        }
    }
    /// Severity-tinted palette for a toast. The accent hue is fixed per level
    /// but blended against this theme's background, so the same request yields
    /// theme-appropriate colours in light and dark.
    pub fn toast(self, level: ToastLevel) -> OverlayPalette {
        let mut palette = self.overlay();
        palette.accent = match level {
            ToastLevel::Info => self.focus,
            ToastLevel::Warning => self.tint(Color(0xF5B76B), 216),
            ToastLevel::Error => self.tint(Color(0xE0605A), 224),
        };
        palette
    }
}
/// Severity of a toast, which selects the accent and the dismissal rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warning,
    Error,
}
/// Colours for a floating overlay, every field derived from a [`UiTheme`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayPalette {
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
}
#[cfg(test)]
mod tests {
    use super::*;
    fn light() -> UiTheme {
        UiTheme {
            editor: Color(0xFFFFFF),
            chrome: Color(0xF3F3F3),
            elevated: Color(0xFAFAFA),
            text: Color(0x1A1A1A),
            muted: Color(0x606060),
            border: Color(0xD0D0D0),
            interactive: Color(0x808080),
            focus: Color(0x1462B8),
            selection: Color(0xCCE4FF),
            caret: Color(0x1462B8),
        }
    }
    #[test]
    fn overlay_palettes_derive_from_ui_theme_in_both_themes() {
        for theme in [UiTheme::default(), light()] {
            let overlay = theme.overlay();
            // Every neutral field is taken directly from the theme.
            assert_eq!(overlay.surface, theme.elevated);
            assert_eq!(overlay.border, theme.border);
            assert_eq!(overlay.text, theme.text);
            assert_eq!(overlay.muted, theme.muted);
            assert_eq!(overlay.accent, theme.focus);
            // The three severities are visibly distinct within a theme.
            let info = theme.toast(ToastLevel::Info).accent;
            let warn = theme.toast(ToastLevel::Warning).accent;
            let error = theme.toast(ToastLevel::Error).accent;
            assert_ne!(info, warn);
            assert_ne!(warn, error);
            assert_ne!(info, error);
            // Neutral fields carry through to the severity palette.
            assert_eq!(theme.toast(ToastLevel::Error).surface, theme.elevated);
        }
        // The same request resolves to different colours per theme, proving the
        // palette is derived from the theme rather than hard-coded.
        let (dark, light) = (UiTheme::default(), light());
        assert_ne!(dark.overlay().surface, light.overlay().surface);
        assert_ne!(
            dark.toast(ToastLevel::Warning).accent,
            light.toast(ToastLevel::Warning).accent
        );
        assert_ne!(
            dark.toast(ToastLevel::Error).accent,
            light.toast(ToastLevel::Error).accent
        );
    }
}
