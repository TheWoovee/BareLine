// SPDX-License-Identifier: MPL-2.0
//! Compile-complete unsupported native adapter; this is not a product port.
use bareline_platform::{Capability, PlatformReadiness, PlatformServices, Unsupported};
use std::path::PathBuf;
#[derive(Default)]
pub struct NativePlatform;
impl PlatformReadiness for NativePlatform {
    fn require(&self, capability: Capability) -> Result<(), Unsupported> {
        Err(Unsupported { capability })
    }
}
impl PlatformServices for NativePlatform {
    // Legacy void API cannot return an error; capability query is mandatory before use.
    fn about(&self) {
        eprintln!(
            "{}",
            Unsupported {
                capability: Capability::About
            }
        );
    }
    fn open_file(&self) -> Result<Option<PathBuf>, String> {
        Err(Unsupported {
            capability: Capability::OpenFile,
        }
        .to_string())
    }
    fn save_file(&self) -> Result<Option<PathBuf>, String> {
        Err(Unsupported {
            capability: Capability::SaveFile,
        }
        .to_string())
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String> {
        Err(Unsupported {
            capability: Capability::PickFolder,
        }
        .to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_remains_usable_when_native_menus_are_unsupported() {
        use bareline_renderer::RenderBackend;
        let adapter = NativePlatform;
        assert!(adapter.require(Capability::MenuBar).is_err());
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        renderer.resize(640, 480, 1.0).unwrap();
        let ops = bareline_ui::shell(640.0, 480.0, &["Untitled".into()], 0, false);
        renderer.render(&ops).unwrap();
        assert!(!renderer.operations.is_empty());
    }
    #[test]
    fn dialogs_and_menus_are_explicitly_unavailable() {
        let adapter = NativePlatform;
        for result in [adapter.open_file(), adapter.save_file(), adapter.pick_folder()] {
            assert!(result.unwrap_err().starts_with("Unsupported:"));
        }
        for capability in [
            Capability::About,
            Capability::MenuBar,
            Capability::ContextMenu,
            Capability::Printing,
            Capability::Tray,
            Capability::Update,
            Capability::Shell,
            Capability::FileWatch,
        ] {
            assert_eq!(adapter.require(capability), Err(Unsupported { capability }));
        }
    }
}
