// SPDX-License-Identifier: MPL-2.0
//! Open, save and folder panels, and alerts for in-app prompts.
//!
//! Main thread only: [`MacDialogs::new`] takes a `MainThreadMarker`. Every
//! panel and alert runs application-modal (`runModal`) and returns when the
//! person answers, as the Windows dialogs do; while it runs, Bareline's menu
//! items are disabled by AppKit's modal rule (see `menu`).
//!
//! Differences from Windows, by necessity:
//! - the save panel always asks before replacing a file
//!   ([`crate::types::NATIVE_OVERWRITE_PROMPT`]);
//! - files chosen in a panel may enter the system's recent places whatever
//!   `set_dialog_recent` says; AppKit has no switch for it;
//! - an alert has no window title, so a task dialog's title is not shown.
use super::ns_string;
use crate::{
    prompts::{AboutAction, AlertPlan, CANCEL_ID, InAppPrompt, PromptStyle, SavePromptOutcome},
    types::{SavePanelPlan, save_panel_plan},
};
use bareline_platform::{PlatformServices, SaveDialogOptions, SaveFileKind};
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSEventModifierFlags, NSModalResponseOK, NSOpenPanel, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};
use std::path::{Path, PathBuf};

pub struct MacDialogs {
    mtm: MainThreadMarker,
}

fn path_of(url: &NSURL) -> Option<PathBuf> {
    // SAFETY: reading a file URL's path.
    unsafe { url.path() }.map(|path| PathBuf::from(path.to_string()))
}
fn directory_url(path: &Path) -> Retained<NSURL> {
    // SAFETY: builds a file URL from an owned string.
    unsafe { NSURL::fileURLWithPath_isDirectory(&ns_string(&path.to_string_lossy()), true) }
}

impl MacDialogs {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self { mtm }
    }

    fn open_panel(&self, multiple: bool, folders: bool) -> Option<Vec<PathBuf>> {
        // SAFETY: a fresh panel configured and run on the main thread.
        unsafe {
            let panel = NSOpenPanel::openPanel(self.mtm);
            panel.setCanChooseFiles(!folders);
            panel.setCanChooseDirectories(folders);
            panel.setCanCreateDirectories(folders);
            panel.setAllowsMultipleSelection(multiple);
            panel.setResolvesAliases(true);
            if panel.runModal() != NSModalResponseOK {
                return None;
            }
            Some(panel.URLs().iter().filter_map(path_of).collect())
        }
    }

    /// Several files to open; empty when cancelled.
    pub fn open_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(self.open_panel(true, false).unwrap_or_default())
    }

    /// A save panel configured from `plan`, not yet shown.
    pub fn save_panel(&self, plan: &SavePanelPlan) -> Retained<NSSavePanel> {
        // SAFETY: a fresh panel configured on the main thread.
        unsafe {
            let panel = NSSavePanel::savePanel(self.mtm);
            panel.setCanCreateDirectories(true);
            panel.setExtensionHidden(false);
            panel.setCanSelectHiddenExtension(true);
            panel.setAllowsOtherFileTypes(plan.allows_other_types);
            let allowed = plan.allowed_extensions.as_ref().map(|extensions| {
                let names: Vec<Retained<NSString>> = extensions.iter().map(|extension| ns_string(extension)).collect();
                NSArray::from_vec(names)
            });
            // `allowedContentTypes` needs UTType objects from another framework
            // binding; the extension list is deprecated since macOS 12 but works
            // on every supported release (13 and later).
            #[allow(deprecated, reason = "functional on macOS 13+, see above")]
            panel.setAllowedFileTypes(allowed.as_deref());
            if let Some(name) = &plan.name {
                panel.setNameFieldStringValue(&ns_string(name));
            }
            let documents = plan
                .documents_fallback
                .then(|| std::env::home_dir().map(|home| home.join("Documents")))
                .flatten();
            if let Some(directory) = plan.directory.as_ref().or(documents.as_ref()) {
                panel.setDirectoryURL(Some(&directory_url(directory)));
            }
            panel
        }
    }

    pub fn save_file_with(&self, options: &SaveDialogOptions) -> Result<Option<PathBuf>, String> {
        let panel = self.save_panel(&save_panel_plan(options));
        // SAFETY: runs the configured panel modally on the main thread.
        unsafe {
            if panel.runModal() != NSModalResponseOK {
                return Ok(None);
            }
            Ok(panel.URL().as_deref().and_then(path_of))
        }
    }

    /// An alert laid out from `plan`, not yet shown.
    pub fn alert(&self, plan: &AlertPlan) -> Retained<NSAlert> {
        // SAFETY: a fresh alert configured on the main thread.
        unsafe {
            let alert = NSAlert::new(self.mtm);
            alert.setMessageText(&ns_string(&plan.message));
            alert.setInformativeText(&ns_string(&plan.detail));
            alert.setAlertStyle(match plan.style {
                PromptStyle::Informational => NSAlertStyle::Informational,
                PromptStyle::Warning => NSAlertStyle::Warning,
                PromptStyle::Critical => NSAlertStyle::Critical,
            });
            for button in &plan.buttons {
                let native = alert.addButtonWithTitle(&ns_string(&button.title));
                if let Some(key) = &button.key {
                    native.setKeyEquivalent(&ns_string(&key.key));
                    native.setKeyEquivalentModifierMask(NSEventModifierFlags(key.modifiers.0 as usize));
                }
            }
            alert
        }
    }

    /// Shows `prompt` and returns the chosen button's id, or [`CANCEL_ID`].
    pub fn prompt(&self, prompt: &InAppPrompt) -> i32 {
        let plan = prompt.plan();
        let alert = self.alert(&plan);
        // SAFETY: runs the configured alert modally on the main thread.
        plan.answer(unsafe { alert.runModal() })
    }

    /// The shell's task dialog; see [`InAppPrompt::task`].
    pub fn task_dialog(
        &self,
        _title: &str,
        instruction: &str,
        content: &str,
        buttons: &[(i32, &str)],
        default_button: i32,
    ) -> i32 {
        self.prompt(&InAppPrompt::task(instruction, content, buttons, default_button))
    }
    /// A question whose action discards or replaces something; Cancel is the default.
    pub fn confirm(&self, message: &str, detail: &str, action: &str) -> bool {
        InAppPrompt::confirmed(self.prompt(&InAppPrompt::confirm(message, detail, action)))
    }
    pub fn about_details(&self, details: &str) -> Result<Option<AboutAction>, String> {
        let prompt = InAppPrompt::about(details)?;
        Ok(InAppPrompt::about_action(self.prompt(&prompt)))
    }
    pub fn confirm_save_document(&self, name: &str) -> SavePromptOutcome {
        InAppPrompt::save_outcome(self.prompt(&InAppPrompt::save_document(name)))
    }
    pub fn confirm_save_all(&self, names: &[String]) -> SavePromptOutcome {
        InAppPrompt::save_outcome(self.prompt(&InAppPrompt::save_all(names)))
    }
    pub fn operation_failed(&self, details: &str) {
        let answer = self.prompt(&InAppPrompt::operation_failed(details));
        debug_assert_eq!(answer, CANCEL_ID);
    }
}

impl PlatformServices for MacDialogs {
    fn about(&self) {
        let details = format!("Version {}", env!("CARGO_PKG_VERSION"));
        // The text is fixed and valid, so the prompt always builds.
        if let Ok(prompt) = InAppPrompt::about(&details) {
            self.prompt(&prompt);
        }
    }
    fn open_file(&self) -> Result<Option<PathBuf>, String> {
        Ok(self.open_panel(false, false).and_then(|paths| paths.into_iter().next()))
    }
    fn save_file(&self) -> Result<Option<PathBuf>, String> {
        self.save_file_with(&SaveDialogOptions::new(SaveFileKind::Any))
    }
    fn save_file_with(&self, options: &SaveDialogOptions) -> Result<Option<PathBuf>, String> {
        MacDialogs::save_file_with(self, options)
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String> {
        Ok(self.open_panel(false, true).and_then(|paths| paths.into_iter().next()))
    }
}
