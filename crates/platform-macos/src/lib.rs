// SPDX-License-Identifier: MPL-2.0
//! macOS platform services (PR-032), next to the compile-complete
//! [`NativePlatform`] stand-in the shell seam still uses until wave 3 wires
//! these in.
//!
//! Portable parts, compiled and tested on every system:
//! - [`keys`]: semantic to physical modifiers (Primary is Command) and keymap
//!   chords to menu key equivalents;
//! - [`menu_plan`]: the menu bar as data, with the HIG application menu;
//! - [`prompts`]: typed in-app prompts and their alert layouts;
//! - [`types`]: pasteboard types, path UTIs, save-panel plans, language tags;
//! - on Unix, `watch_snapshot` (directory diffs in the Windows watch
//!   vocabulary) and [`isolation_policy`] (sandbox profile and launch chain).
//!
//! macOS only, re-exported at the root: [`MacMenuBar`], [`MacClipboard`],
//! [`MacDialogs`], [`MacAppearance`], [`MacWatchService`], [`MacIsolation`]
//! and [`peer`]. Every AppKit type is created and used on the main thread:
//! constructors take a `MainThreadMarker`, which only the main thread can
//! obtain, and the values are not `Send`, so the compiler enforces the rule.
//! The watcher, the clipboard's limits and isolation have no thread rule.
use bareline_platform::{Capability, PlatformReadiness, PlatformServices, Unsupported};
/// Filesystem, path trust, capability and data-folder services (PR-030).
#[cfg(unix)]
pub use bareline_platform_posix::{
    DirectoryGuard, PosixFileSystem, PosixFilesystemCapability, PosixPathTrustProvider, PosixSessionPathTrustProvider,
    paths,
};
use std::path::PathBuf;

#[cfg(unix)]
pub mod isolation_policy;
pub mod keys;
pub mod menu_plan;
pub mod prompts;
pub mod types;
// Used by the macOS watcher; built on other Unix systems for its tests.
#[cfg(all(unix, any(target_os = "macos", test)))]
mod watch_snapshot;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    appearance::{Appearance, MacAppearance, high_contrast_enabled, system_ui_language},
    clipboard::MacClipboard,
    dialogs::MacDialogs,
    isolation::{IsolationSupport, IsolationUnavailable, MacHostChild, MacIsolation},
    menu::{MacMenuBar, MenuDispatch},
    peer,
    watch::MacWatchService,
};
pub use menu_plan::CommandMessage;
/// The proof of running on the main thread that every AppKit constructor
/// takes, so the shell's seam need not depend on objc2 itself.
#[cfg(target_os = "macos")]
pub use objc2_foundation::MainThreadMarker;
pub use prompts::{AboutAction, SaveChoice, SavePromptOutcome};

/// The shell's stand-in until wave 3 wires the services above into the seam:
/// every capability reports Unsupported.
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
