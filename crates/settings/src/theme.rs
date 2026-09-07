// SPDX-License-Identifier: MPL-2.0
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemAppearance {
    pub dark: bool,
    pub high_contrast: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColor {
    pub rgb: u32,
    pub alpha: u8,
}
impl ThemeColor {
    pub const fn opaque(rgb: u32) -> Self {
        Self { rgb, alpha: 255 }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        let hex = value
            .strip_prefix('#')
            .ok_or("Colors require #RRGGBB or #RRGGBBAA")?;
        if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
            return Err("Colors require #RRGGBB or #RRGGBBAA".into());
        }
        let n = u32::from_str_radix(hex, 16).map_err(|_| "Invalid hexadecimal color")?;
        Ok(if hex.len() == 6 {
            Self::opaque(n)
        } else {
            Self {
                rgb: n >> 8,
                alpha: n as u8,
            }
        })
    }
    pub fn composite(self, background: Self) -> Self {
        let alpha = self.alpha as f64 / 255.0;
        let mut rgb = 0;
        for shift in [16, 8, 0] {
            let fg = ((self.rgb >> shift) & 255) as f64;
            let bg = ((background.rgb >> shift) & 255) as f64;
            rgb |= ((fg * alpha + bg * (1.0 - alpha)).round() as u32) << shift;
        }
        Self::opaque(rgb)
    }
    pub fn contrast(self, background: Self) -> f64 {
        fn luminance(color: ThemeColor) -> f64 {
            [16, 8, 0]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(shift, weight)| {
                    let c = ((color.rgb >> shift) & 255) as f64 / 255.0;
                    weight
                        * if c <= 0.04045 {
                            c / 12.92
                        } else {
                            ((c + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum()
        }
        let a = luminance(self.composite(background));
        let b = luminance(background);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}
pub static TOKEN_NAMES: &[&str] = &[
    "surface",
    "surface.editor",
    "surface.chrome",
    "surface.elevated",
    "surface.currentLine",
    "text",
    "text.muted",
    "text.gutter",
    "accent",
    "accent.text",
    "selection",
    "caret",
    "border",
    "border.interactive",
    "focus.ring",
    "danger",
    "warning",
    "success",
    "diff.added",
    "diff.removed",
    "diff.changed",
    "diff.moved",
    "diff.current",
    "diff.added.gutter",
    "diff.removed.gutter",
    "diff.changed.gutter",
    "diff.moved.gutter",
    "diff.current.gutter",
    "diff.added.overview",
    "diff.removed.overview",
    "diff.changed.overview",
    "diff.moved.overview",
    "diff.current.overview",
    "syntax.keyword",
    "syntax.string",
    "syntax.number",
    "syntax.comment",
    "syntax.function",
    "syntax.type",
    "syntax.operator",
    "mark.style1",
    "mark.style2",
    "mark.style3",
    "mark.style4",
    "mark.style5",
];
#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    pub high_contrast: bool,
    pub tokens: BTreeMap<String, ThemeColor>,
}
impl Theme {
    pub fn resolve(
        mode: ThemeMode,
        system: SystemAppearance,
        overrides: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let dark = match mode {
            ThemeMode::System => system.dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        };
        let mut theme = Self::builtin(dark);
        for (key, value) in overrides {
            if !TOKEN_NAMES.contains(&key.as_str()) {
                return Err(format!("Unknown theme token: {key}"));
            }
            theme.tokens.insert(key.clone(), ThemeColor::parse(value)?);
        }
        if system.high_contrast {
            // Functional high contrast deliberately overrides decorative user colors.
            theme.high_contrast = true;
            let bg = ThemeColor::opaque(if dark { 0x000000 } else { 0xffffff });
            let fg = ThemeColor::opaque(if dark { 0xffffff } else { 0x000000 });
            let focus = ThemeColor::opaque(if dark { 0xffff00 } else { 0x000080 });
            for key in TOKEN_NAMES {
                let value = if key.starts_with("surface")
                    || key.starts_with("diff.")
                        && !key.ends_with("gutter")
                        && !key.ends_with("overview")
                    || key.starts_with("mark.")
                {
                    bg
                } else if *key == "focus.ring" || *key == "caret" {
                    focus
                } else if *key == "selection" {
                    ThemeColor {
                        rgb: focus.rgb,
                        alpha: 32,
                    }
                } else {
                    fg
                };
                theme.tokens.insert((*key).into(), value);
            }
        }
        theme.validate_contrast()?;
        Ok(theme)
    }
    pub fn color(&self, key: &str) -> Option<ThemeColor> {
        self.tokens.get(key).copied()
    }
    pub fn builtin(dark: bool) -> Self {
        let mut tokens = BTreeMap::new();
        for (key, light, dark_color) in [
            ("surface", 0xFAF8F5, 0x1F2328),
            ("surface.editor", 0xFAF8F5, 0x1F2328),
            ("surface.chrome", 0xF1EEE9, 0x181B1F),
            ("surface.elevated", 0xFFFFFF, 0x262B31),
            ("surface.currentLine", 0xF3F0EB, 0x262B31),
            ("text", 0x23272B, 0xE6E8EA),
            ("text.muted", 0x5C6570, 0x9AA3AD),
            ("text.gutter", 0x5C6570, 0x9AA3AD),
            ("accent", 0x14A89A, 0x2ED3C4),
            ("accent.text", 0x0B7A6F, 0x2ED3C4),
            ("caret", 0x0B7A6F, 0x2ED3C4),
            ("border", 0xDDD8D0, 0x343A42),
            ("border.interactive", 0x5C6570, 0x9AA3AD),
            ("focus.ring", 0x0B7A6F, 0x2ED3C4),
            ("danger", 0xC4463D, 0xF0705F),
            ("warning", 0xB7791F, 0xE3B341),
            ("success", 0x2E8B57, 0x5FCF80),
            ("diff.added", 0xE3F5EA, 0x1E3A2F),
            ("diff.removed", 0xFBE4E4, 0x3D2426),
            ("diff.changed", 0xFFF3D6, 0x3A3420),
            ("diff.moved", 0xE6EEF8, 0x23303F),
            ("diff.current", 0x0B7A6F, 0x2ED3C4),
            ("syntax.keyword", 0x7B3FB5, 0xC79BFF),
            ("syntax.string", 0x2E7D32, 0xA5D6A7),
            ("syntax.number", 0xB45309, 0xF5B76B),
            ("syntax.comment", 0x5C6570, 0x9AA3AD),
            ("syntax.function", 0x0B63C5, 0x8AB4F8),
            ("syntax.type", 0x0B7A6F, 0x2ED3C4),
            ("syntax.operator", 0x23272B, 0xE6E8EA),
            ("mark.style1", 0xE3F5EA, 0x1E3A2F),
            ("mark.style2", 0xFFF3D6, 0x3A3420),
            ("mark.style3", 0xE6EEF8, 0x23303F),
            ("mark.style4", 0xFBE4E4, 0x3D2426),
            ("mark.style5", 0xEEE5F6, 0x342740),
        ] {
            tokens.insert(
                key.into(),
                ThemeColor::opaque(if dark { dark_color } else { light }),
            );
        }
        tokens.insert(
            "selection".into(),
            ThemeColor {
                rgb: if dark { 0x2ED3C4 } else { 0x14A89A },
                alpha: if dark { 56 } else { 51 },
            },
        );
        for name in ["added", "removed", "changed", "moved", "current"] {
            let color = tokens[if name == "current" {
                "focus.ring"
            } else {
                "text"
            }];
            for variant in ["gutter", "overview"] {
                tokens.insert(format!("diff.{name}.{variant}"), color);
            }
        }
        Self {
            dark,
            high_contrast: false,
            tokens,
        }
    }
    pub fn validate_contrast(&self) -> Result<(), String> {
        let editor = self.tokens["surface.editor"];
        for surface in [
            "surface",
            "surface.editor",
            "surface.chrome",
            "surface.elevated",
            "surface.currentLine",
            "diff.added",
            "diff.removed",
            "diff.changed",
            "diff.moved",
            "mark.style1",
            "mark.style2",
            "mark.style3",
            "mark.style4",
            "mark.style5",
        ] {
            let bg = self.tokens[surface].composite(editor);
            self.check_pair("text", bg, 4.5, surface)?;
            self.check_pair("focus.ring", bg, 3.0, surface)?;
            self.check_pair("caret", bg, 3.0, surface)?;
        }
        let selection = self.tokens["selection"].composite(editor);
        self.check_pair("text", selection, 4.5, "selection composite")?;
        self.check_pair("focus.ring", selection, 3.0, "selection composite")?;
        for key in [
            "text.muted",
            "text.gutter",
            "accent.text",
            "syntax.keyword",
            "syntax.string",
            "syntax.number",
            "syntax.comment",
            "syntax.function",
            "syntax.type",
            "syntax.operator",
        ] {
            self.check_pair(key, editor, 4.5, "surface.editor")?;
        }
        for surface in ["surface.editor", "surface.chrome", "surface.elevated"] {
            self.check_pair(
                "border.interactive",
                self.tokens[surface].composite(editor),
                3.0,
                surface,
            )?;
        }
        Ok(())
    }
    fn check_pair(
        &self,
        key: &str,
        bg: ThemeColor,
        minimum: f64,
        surface: &str,
    ) -> Result<(), String> {
        let ratio = self.tokens[key].contrast(bg);
        if ratio < minimum {
            return Err(format!(
                "{key} contrast on {surface} is {ratio:.2}:1; requires {minimum}:1"
            ));
        }
        Ok(())
    }
}
