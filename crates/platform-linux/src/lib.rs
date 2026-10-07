// SPDX-License-Identifier: MPL-2.0
//! The Linux native adapter. Filesystem, path trust, capability and data-folder
//! services (PR-030) and the platform services of PR-031 are real library types:
//! the clipboard, portal dialogs, in-app prompts, inotify file watching, the
//! single-instance handoff, the extension host transport and its Landlock
//! sandbox, and the desktop appearance. `NativePlatform` still answers
//! `Unsupported` until the shell's seam wires those types in.
use bareline_platform::{Capability, PlatformReadiness, PlatformServices, Unsupported};
#[cfg(unix)]
pub use bareline_platform_posix::{
    DirectoryGuard, PosixFileSystem, PosixFilesystemCapability, PosixPathTrustProvider, PosixSessionPathTrustProvider,
    instance, paths,
};

#[cfg(target_os = "linux")]
pub mod appearance;
#[cfg(target_os = "linux")]
pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod dialogs;
#[cfg(target_os = "linux")]
pub mod extension_transport;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
pub mod prompts;
#[cfg(target_os = "linux")]
pub mod sandbox;
#[cfg(target_os = "linux")]
mod watch;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
pub use {
    appearance::{LinuxAppearance, spell_checker_factory, system_ui_language},
    clipboard::LinuxClipboard,
    dialogs::LinuxDialogs,
    prompts::{AboutAction, InAppPrompt, SaveChoice, SavePromptOutcome},
    sandbox::LinuxHostSandbox,
    watch::LinuxWatchService,
};
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
        // The dialogs report the capability's plain-language refusal (UI-03), so
        // callers show an explicit "not supported" message rather than a failure.
        for (result, capability) in [
            (adapter.open_file(), Capability::OpenFile),
            (adapter.save_file(), Capability::SaveFile),
            (adapter.pick_folder(), Capability::PickFolder),
        ] {
            let message = result.unwrap_err();
            assert_eq!(message, Unsupported { capability }.to_string());
            assert!(message.starts_with("This system does not support "), "{message}");
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
