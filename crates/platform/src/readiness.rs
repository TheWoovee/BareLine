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
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unsupported: {:?}", self.capability)
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
pub fn map_modifier(modifier: SemanticModifier, platform: DesktopPlatform) -> PhysicalModifier {
    match modifier {
        SemanticModifier::Primary if platform == DesktopPlatform::MacOs => PhysicalModifier::Super,
        SemanticModifier::Primary => PhysicalModifier::Control,
        SemanticModifier::Alt => PhysicalModifier::Alt,
        SemanticModifier::Shift => PhysicalModifier::Shift,
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
        }
    }
}
