// SPDX-License-Identifier: MPL-2.0
//! Keyboard conventions on macOS: semantic modifiers become physical keys, and
//! a keymap chord becomes an AppKit menu key equivalent.
//!
//! Bareline stores chords in Windows notation: `Ctrl` is the primary modifier
//! and `Meta` is the Windows key. On macOS the primary modifier is Command,
//! Alt is Option and the Super role moves to Control, so `Ctrl+S` becomes ⌘S
//! and `Ctrl+Alt+Up` becomes ⌥⌘↑. Everything here is plain Rust so it is
//! tested on every system.
use bareline_commands::{Key, KeyChord};
use bareline_platform::{DesktopPlatform, PhysicalModifier, SemanticModifier, map_modifier};

/// The physical key a semantic modifier is held with on macOS: Primary is
/// Command (`PhysicalModifier::Super`), Alt is Option (`PhysicalModifier::Alt`),
/// Shift is Shift and Super is Control.
pub fn macos_modifier(modifier: SemanticModifier) -> PhysicalModifier {
    map_modifier(modifier, DesktopPlatform::MacOs)
}

/// `NSEventModifierFlags` bits, kept as plain numbers so menu plans are built
/// and tested without AppKit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModifierMask(pub u64);
impl ModifierMask {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 17);
    pub const CONTROL: Self = Self(1 << 18);
    pub const OPTION: Self = Self(1 << 19);
    pub const COMMAND: Self = Self(1 << 20);
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// The macOS key a physical modifier names: `Super` is the Command key there.
pub fn physical_mask(modifier: PhysicalModifier) -> ModifierMask {
    match modifier {
        PhysicalModifier::Control => ModifierMask::CONTROL,
        PhysicalModifier::Alt => ModifierMask::OPTION,
        PhysicalModifier::Shift => ModifierMask::SHIFT,
        PhysicalModifier::Super => ModifierMask::COMMAND,
    }
}

/// A menu item's key equivalent: the `keyEquivalent` string and its
/// `keyEquivalentModifierMask`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyEquivalent {
    pub key: String,
    pub modifiers: ModifierMask,
}
impl KeyEquivalent {
    pub fn new(key: impl Into<String>, modifiers: ModifierMask) -> Self {
        Self {
            key: key.into(),
            modifiers,
        }
    }
}

/// AppKit function-key characters (`NSUpArrowFunctionKey` and friends).
const UP: char = '\u{f700}';
const DOWN: char = '\u{f701}';
const LEFT: char = '\u{f702}';
const RIGHT: char = '\u{f703}';
const F1: u32 = 0xf704;
const INSERT: char = '\u{f727}';
const FORWARD_DELETE: char = '\u{f728}';
const HOME: char = '\u{f729}';
const END: char = '\u{f72b}';
const PAGE_UP: char = '\u{f72c}';
const PAGE_DOWN: char = '\u{f72d}';
const PRINT_SCREEN: char = '\u{f72e}';
const SCROLL_LOCK: char = '\u{f72f}';
const PAUSE: char = '\u{f730}';
const MENU: char = '\u{f735}';
const BACKSPACE: char = '\u{8}';
const ESCAPE: char = '\u{1b}';

/// The key-equivalent text for a stored logical key name, and whether it is a
/// function key (F1 to F35), which may stand alone as a menu shortcut.
fn key_text(name: &str) -> Option<(String, bool)> {
    let mut characters = name.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        if character.is_control() {
            return None;
        }
        // AppKit matches letters case-sensitively; Shift is carried by the mask.
        return Some((character.to_lowercase().collect(), false));
    }
    if let Some(number) = name.strip_prefix('F').and_then(|number| number.parse::<u32>().ok())
        && (1..=35).contains(&number)
    {
        return char::from_u32(F1 + number - 1).map(|key| (key.to_string(), true));
    }
    let key = match name {
        "SPACE" => ' ',
        "TAB" => '\t',
        "ENTER" | "RETURN" => '\r',
        "ESCAPE" => ESCAPE,
        "BACKSPACE" => BACKSPACE,
        "DELETE" => FORWARD_DELETE,
        "INSERT" => INSERT,
        "HOME" => HOME,
        "END" => END,
        "PAGEUP" => PAGE_UP,
        "PAGEDOWN" => PAGE_DOWN,
        "UP" => UP,
        "DOWN" => DOWN,
        "LEFT" => LEFT,
        "RIGHT" => RIGHT,
        "PRINTSCREEN" => PRINT_SCREEN,
        "SCROLLLOCK" => SCROLL_LOCK,
        "PAUSE" => PAUSE,
        "CONTEXTMENU" => MENU,
        // Caps Lock and Num Lock are not menu keys on a Mac keyboard.
        _ => return None,
    };
    Some((key.to_string(), false))
}

/// The menu key equivalent for one chord, or `None` when the chord cannot or
/// must not be a menu shortcut:
/// - physical-key bindings name a key position, which a key equivalent cannot;
/// - a key without Command or Control (bare, Shift or Option only) stays with
///   typing and editing, except the function keys: Option with a character
///   key types text on a Mac (Option+E is a dead key), and a menu that took a
///   bare Tab, Enter or arrow would take it from the editor.
pub fn key_equivalent(chord: &KeyChord) -> Option<KeyEquivalent> {
    let Key::Logical(name) = &chord.key else {
        return None;
    };
    let (key, function) = key_text(name)?;
    let modifiers = [
        (chord.ctrl, SemanticModifier::Primary),
        (chord.alt, SemanticModifier::Alt),
        (chord.shift, SemanticModifier::Shift),
        (chord.meta, SemanticModifier::Super),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(ModifierMask::NONE, |mask, (_, modifier)| {
        mask.union(physical_mask(macos_modifier(modifier)))
    });
    let commanding = modifiers.intersects(ModifierMask::COMMAND.union(ModifierMask::CONTROL));
    let named_with_option = modifiers.intersects(ModifierMask::OPTION) && !chord.produces_text();
    (commanding || named_with_option || function).then_some(KeyEquivalent { key, modifiers })
}

/// A key equivalent as macOS writes it, for shortcut labels outside the menu
/// bar (the palette, tooltips): modifiers in Apple's order ⌃⌥⇧⌘, then the key.
pub fn shortcut_label(equivalent: &KeyEquivalent) -> String {
    let mut label = String::new();
    for (mask, symbol) in [
        (ModifierMask::CONTROL, '⌃'),
        (ModifierMask::OPTION, '⌥'),
        (ModifierMask::SHIFT, '⇧'),
        (ModifierMask::COMMAND, '⌘'),
    ] {
        if equivalent.modifiers.contains(mask) {
            label.push(symbol);
        }
    }
    let mut characters = equivalent.key.chars();
    let key = match (characters.next(), characters.next()) {
        (Some(key), None) => key,
        _ => {
            label.push_str(&equivalent.key);
            return label;
        }
    };
    match key {
        UP => label.push('↑'),
        DOWN => label.push('↓'),
        LEFT => label.push('←'),
        RIGHT => label.push('→'),
        HOME => label.push('↖'),
        END => label.push('↘'),
        PAGE_UP => label.push('⇞'),
        PAGE_DOWN => label.push('⇟'),
        FORWARD_DELETE => label.push('⌦'),
        BACKSPACE => label.push('⌫'),
        ESCAPE => label.push('⎋'),
        '\r' => label.push('↩'),
        '\t' => label.push('⇥'),
        ' ' => label.push_str("Space"),
        key if (F1..F1 + 35).contains(&u32::from(key)) => {
            label.push_str(&format!("F{}", u32::from(key) - F1 + 1));
        }
        key => label.extend(key.to_uppercase()),
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equivalent(chord: &str) -> Option<KeyEquivalent> {
        key_equivalent(&KeyChord::parse(chord).unwrap())
    }

    #[test]
    fn semantic_modifiers_follow_mac_keyboard_conventions() {
        assert_eq!(macos_modifier(SemanticModifier::Primary), PhysicalModifier::Super);
        assert_eq!(macos_modifier(SemanticModifier::Alt), PhysicalModifier::Alt);
        assert_eq!(macos_modifier(SemanticModifier::Shift), PhysicalModifier::Shift);
        assert_eq!(macos_modifier(SemanticModifier::Super), PhysicalModifier::Control);
        assert_eq!(
            physical_mask(macos_modifier(SemanticModifier::Primary)),
            ModifierMask::COMMAND
        );
        assert_eq!(
            physical_mask(macos_modifier(SemanticModifier::Alt)),
            ModifierMask::OPTION
        );
        assert_eq!(
            physical_mask(macos_modifier(SemanticModifier::Super)),
            ModifierMask::CONTROL
        );
    }

    #[test]
    fn windows_chords_become_command_key_equivalents() {
        assert_eq!(
            equivalent("Ctrl+S"),
            Some(KeyEquivalent::new("s", ModifierMask::COMMAND))
        );
        assert_eq!(
            equivalent("Ctrl+Shift+S"),
            Some(KeyEquivalent::new(
                "s",
                ModifierMask::COMMAND.union(ModifierMask::SHIFT)
            ))
        );
        assert_eq!(
            equivalent("Ctrl+Alt+Up"),
            Some(KeyEquivalent::new(
                "\u{f700}",
                ModifierMask::COMMAND.union(ModifierMask::OPTION)
            ))
        );
        assert_eq!(
            equivalent("Meta+Tab"),
            Some(KeyEquivalent::new("\t", ModifierMask::CONTROL))
        );
        assert_eq!(
            equivalent("Ctrl+F12"),
            Some(KeyEquivalent::new("\u{f70f}", ModifierMask::COMMAND))
        );
        assert_eq!(
            equivalent("Ctrl+Delete"),
            Some(KeyEquivalent::new("\u{f728}", ModifierMask::COMMAND))
        );
        assert_eq!(
            equivalent("Ctrl+/"),
            Some(KeyEquivalent::new("/", ModifierMask::COMMAND))
        );
    }

    #[test]
    fn typing_keys_and_key_positions_never_become_menu_shortcuts() {
        // Function keys may stand alone.
        assert_eq!(
            equivalent("F1"),
            Some(KeyEquivalent::new("\u{f704}", ModifierMask::NONE))
        );
        assert_eq!(
            equivalent("Shift+F3"),
            Some(KeyEquivalent::new("\u{f706}", ModifierMask::SHIFT))
        );
        assert_eq!(
            equivalent("Alt+F4"),
            Some(KeyEquivalent::new("\u{f707}", ModifierMask::OPTION))
        );
        // Option with a named key is a shortcut; Option with a character types it.
        assert_eq!(
            equivalent("Alt+Up"),
            Some(KeyEquivalent::new("\u{f700}", ModifierMask::OPTION))
        );
        for chord in [
            "Alt+E",
            "Alt+Shift+2",
            "Tab",
            "Shift+Tab",
            "Enter",
            "Delete",
            "Shift+Left",
            "A",
        ] {
            assert_eq!(equivalent(chord), None, "{chord}");
        }
        assert_eq!(equivalent("Ctrl+Physical:KeyS"), None);
        assert_eq!(equivalent("Ctrl+CapsLock"), None);
    }

    #[test]
    fn labels_use_apple_modifier_order_and_glyphs() {
        let label = |chord| shortcut_label(&equivalent(chord).unwrap());
        assert_eq!(label("Ctrl+S"), "⌘S");
        assert_eq!(label("Ctrl+Shift+Alt+Meta+Z"), "⌃⌥⇧⌘Z");
        assert_eq!(label("Ctrl+Alt+Down"), "⌥⌘↓");
        assert_eq!(label("F11"), "F11");
        assert_eq!(label("Ctrl+Backspace"), "⌘⌫");
        assert_eq!(label("Ctrl+Space"), "⌘Space");
        assert_eq!(label("Ctrl+PageDown"), "⌘⇟");
    }
}
