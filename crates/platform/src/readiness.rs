// SPDX-License-Identifier: MPL-2.0
//! Additive port readiness API. Unsupported is never reported as cancellation/success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    About,
    OpenFile,
    SaveFile,
    PickFolder,
    MenuBar,
    ContextMenu,
    Shell,
    Printing,
    Tray,
    Update,
    FileWatch,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub capability: Capability,
}
/// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let feature = match self.capability {
            Capability::About => "the About window",
            Capability::OpenFile => "the Open dialog",
            Capability::SaveFile => "the Save dialog",
            Capability::PickFolder => "the folder picker",
            Capability::MenuBar => "the menu bar",
            Capability::ContextMenu => "context menus",
            Capability::Shell => "running programs",
            Capability::Printing => "printing",
            Capability::Tray => "the notification area icon",
            Capability::Update => "updates",
            Capability::FileWatch => "watching files for changes",
        };
        write!(f, "This system does not support {feature}")
    }
}
impl std::error::Error for Unsupported {}
pub trait PlatformReadiness {
    fn require(&self, capability: Capability) -> Result<(), Unsupported>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopPlatform {
    Windows,
    Linux,
    MacOs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticModifier {
    Primary,
    Alt,
    Shift,
    Super,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalModifier {
    Control,
    Alt,
    Shift,
    Super,
}
/// The key each semantic modifier is held with. On macOS the primary modifier is
/// Command (`PhysicalModifier::Super`), so the Super role moves to Control and
/// no two semantic modifiers share a key.
pub fn map_modifier(modifier: SemanticModifier, platform: DesktopPlatform) -> PhysicalModifier {
    match modifier {
        SemanticModifier::Primary if platform == DesktopPlatform::MacOs => PhysicalModifier::Super,
        SemanticModifier::Primary => PhysicalModifier::Control,
        SemanticModifier::Alt => PhysicalModifier::Alt,
        SemanticModifier::Shift => PhysicalModifier::Shift,
        SemanticModifier::Super if platform == DesktopPlatform::MacOs => PhysicalModifier::Control,
        SemanticModifier::Super => PhysicalModifier::Super,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primary_follows_native_keyboard_conventions() {
        assert_eq!(
            map_modifier(SemanticModifier::Primary, DesktopPlatform::MacOs),
            PhysicalModifier::Super
        );
        for platform in [DesktopPlatform::Windows, DesktopPlatform::Linux] {
            assert_eq!(
                map_modifier(SemanticModifier::Primary, platform),
                PhysicalModifier::Control
            );
            assert_eq!(map_modifier(SemanticModifier::Super, platform), PhysicalModifier::Super);
        }
    }
    #[test]
    fn every_platform_gives_each_semantic_modifier_its_own_key() {
        let semantic = [
            SemanticModifier::Primary,
            SemanticModifier::Alt,
            SemanticModifier::Shift,
            SemanticModifier::Super,
        ];
        for platform in [DesktopPlatform::Windows, DesktopPlatform::Linux, DesktopPlatform::MacOs] {
            let keys: Vec<_> = semantic.iter().map(|m| map_modifier(*m, platform)).collect();
            for (index, key) in keys.iter().enumerate() {
                assert!(!keys[index + 1..].contains(key), "{platform:?} reuses {key:?}");
            }
        }
        // Command is primary on macOS, so Super is held with Control there.
        assert_eq!(
            map_modifier(SemanticModifier::Super, DesktopPlatform::MacOs),
            PhysicalModifier::Control
        );
    }
    #[test]
    fn unsupported_names_the_feature_in_plain_language() {
        for capability in [
            Capability::About,
            Capability::OpenFile,
            Capability::SaveFile,
            Capability::PickFolder,
            Capability::MenuBar,
            Capability::ContextMenu,
            Capability::Shell,
            Capability::Printing,
            Capability::Tray,
            Capability::Update,
            Capability::FileWatch,
        ] {
            let message = Unsupported { capability }.to_string();
            assert!(message.starts_with("This system does not support "), "{message}");
            // No variant name such as `OpenFile` or `FileWatch` reaches the user.
            assert!(
                !message
                    .chars()
                    .zip(message.chars().skip(1))
                    .any(|(first, second)| first.is_lowercase() && second.is_uppercase()),
                "{message}"
            );
        }
    }
}
