// SPDX-License-Identifier: MPL-2.0
//! Typed in-app prompts and their `NSAlert` plans: the About box, "save
//! changes?" questions, confirmations and the shell's general task dialog.
//!
//! The answers mirror the Windows adapter's: a button id from the caller's
//! list, [`CANCEL_ID`] (Windows `IDCANCEL`) for Cancel and Escape, the
//! [`SaveChoice`] of a save prompt and the [`AboutAction`] of the About box.
//! macOS conventions apply to the layout: the first button is the default
//! (Return) and sits rightmost, Cancel answers Escape, "Don't Save" answers ⌘D,
//! and a destructive confirmation keeps Cancel as its default.
use crate::keys::{KeyEquivalent, ModifierMask};
use crate::menu_plan::plain_title;

/// The Windows `IDCANCEL` answer, which the shell treats as a cancelled dialog.
pub const CANCEL_ID: i32 = 2;
pub const SAVE_ID: i32 = 1101;
pub const DONT_SAVE_ID: i32 = 1102;
const LICENSE_ID: i32 = 1001;
const NOTICES_ID: i32 = 1002;
const DIAGNOSTICS_ID: i32 = 1003;
const CONFIRM_ID: i32 = 1103;
/// `NSAlertFirstButtonReturn`; later buttons answer 1001, 1002 and so on.
pub const FIRST_BUTTON_RETURN: isize = 1000;
/// The longest About text accepted, as on Windows.
const MAX_ABOUT_BYTES: usize = 16 * 1024;
/// Names listed in the exit prompt before the rest are summarized (UI-18).
const SAVE_ALL_LISTED: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutAction {
    License,
    ThirdPartyNotices,
    CopyDiagnostics,
}
/// Answer to a "save changes?" prompt; `Cancel` also covers Escape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePromptOutcome {
    Choice { choice: SaveChoice, selected: i32 },
    Failure(i32),
}
impl SaveChoice {
    pub fn from_id(id: i32) -> Self {
        match id {
            SAVE_ID => Self::Save,
            DONT_SAVE_ID => Self::DontSave,
            _ => Self::Cancel,
        }
    }
}

/// `NSAlertStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptStyle {
    Informational,
    Warning,
    Critical,
}

/// What a prompt asks. Built with the constructors below; [`InAppPrompt::plan`]
/// lays it out for `NSAlert` and [`AlertPlan::answer`] reads the response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InAppPrompt {
    pub style: PromptStyle,
    /// The bold message (`messageText`).
    pub message: String,
    /// The explanation under it (`informativeText`).
    pub detail: String,
    /// Buttons in macOS order: the first is the default.
    pub buttons: Vec<(i32, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertButton {
    pub title: String,
    pub id: i32,
    pub key: Option<KeyEquivalent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertPlan {
    pub style: PromptStyle,
    pub message: String,
    pub detail: String,
    /// In `addButtonWithTitle:` order.
    pub buttons: Vec<AlertButton>,
}
impl AlertPlan {
    /// The caller's id for an `NSModalResponse`; anything that is not one of
    /// the buttons counts as Cancel.
    pub fn answer(&self, response: isize) -> i32 {
        response
            .checked_sub(FIRST_BUTTON_RETURN)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.buttons.get(index))
            .map_or(CANCEL_ID, |button| button.id)
    }
}

/// Tab titles carry the unsaved marker; prose in a prompt must not repeat it.
pub fn display_title(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches(['\u{2022}', '*', '\u{25cf}']).trim();
    if trimmed.is_empty() {
        "this document".to_string()
    } else {
        trimmed.to_string()
    }
}

impl InAppPrompt {
    /// The shell's task dialog: its custom buttons with `default_button` first,
    /// then Cancel. A title is not shown: alerts on macOS have none.
    pub fn task(message: &str, detail: &str, buttons: &[(i32, &str)], default_button: i32) -> Self {
        let mut ordered: Vec<(i32, String)> = buttons
            .iter()
            .filter(|(id, _)| *id == default_button)
            .chain(buttons.iter().filter(|(id, _)| *id != default_button))
            .filter(|(id, _)| *id != CANCEL_ID)
            .map(|(id, label)| (*id, plain_title(label)))
            .collect();
        let cancel = (CANCEL_ID, "Cancel".to_owned());
        if default_button == CANCEL_ID {
            ordered.insert(0, cancel);
        } else {
            ordered.push(cancel);
        }
        Self {
            style: PromptStyle::Informational,
            message: message.to_owned(),
            detail: detail.to_owned(),
            buttons: ordered,
        }
    }
    /// A yes-or-cancel question whose action discards or replaces something:
    /// Cancel is the default, so Return never confirms it by accident.
    pub fn confirm(message: &str, detail: &str, action: &str) -> Self {
        Self {
            style: PromptStyle::Warning,
            ..Self::task(message, detail, &[(CONFIRM_ID, action)], CANCEL_ID)
        }
    }
    /// Whether an answer to [`Self::confirm`] confirmed it.
    pub fn confirmed(answer: i32) -> bool {
        answer == CONFIRM_ID
    }
    /// The About box: metadata supplied by the application, never document
    /// contents, with buttons that return an [`AboutAction`].
    pub fn about(details: &str) -> Result<Self, String> {
        if details.len() > MAX_ABOUT_BYTES || details.contains('\0') {
            return Err("Invalid About metadata".into());
        }
        Ok(Self {
            style: PromptStyle::Informational,
            message: "Bareline".into(),
            detail: format!(
                "{details}\n\nPlain text. Full power. No weight.\nCore: MPL-2.0 \u{b7} Extension SDK: MIT OR Apache-2.0\nPrivacy policy: https://github.com/TheWoovee/BareLine/blob/master/PRIVACY.md"
            ),
            buttons: vec![
                (CANCEL_ID, "Close".into()),
                (LICENSE_ID, "License".into()),
                (NOTICES_ID, "Third-Party Notices".into()),
                (DIAGNOSTICS_ID, "Copy Diagnostics".into()),
            ],
        })
    }
    pub fn about_action(answer: i32) -> Option<AboutAction> {
        match answer {
            LICENSE_ID => Some(AboutAction::License),
            NOTICES_ID => Some(AboutAction::ThirdPartyNotices),
            DIAGNOSTICS_ID => Some(AboutAction::CopyDiagnostics),
            _ => None,
        }
    }
    /// Save / Don't Save / Cancel for one document that is about to close.
    pub fn save_document(name: &str) -> Self {
        Self::save(
            format!("Save changes to {}?", display_title(name)),
            "Your changes will be lost if you don't save them.".into(),
            "Save",
        )
    }
    /// Save All / Don't Save / Cancel on exit, listing the unsaved documents.
    pub fn save_all(names: &[String]) -> Self {
        if let [name] = names {
            // One document gets a plain Save, not "Save All" (UI-21).
            return Self::save_document(name);
        }
        let mut list = names
            .iter()
            .take(SAVE_ALL_LISTED)
            .map(|name| format!("\u{2022} {}", display_title(name)))
            .collect::<Vec<_>>()
            .join("\n");
        let more = names.len().saturating_sub(SAVE_ALL_LISTED);
        if more > 0 {
            list.push_str(&format!("\n\u{2022} and {more} more"));
        }
        Self::save(
            format!("Save changes to {} documents?", names.len()),
            format!(
                "These documents have unsaved changes:\n\n{list}\n\nYour changes will be lost if you don't save them."
            ),
            "Save All",
        )
    }
    fn save(message: String, detail: String, save: &str) -> Self {
        Self {
            style: PromptStyle::Warning,
            message,
            detail,
            buttons: vec![
                (SAVE_ID, save.into()),
                (DONT_SAVE_ID, "Don't Save".into()),
                (CANCEL_ID, "Cancel".into()),
            ],
        }
    }
    /// The answer to a save prompt as the shell's outcome type.
    pub fn save_outcome(answer: i32) -> SavePromptOutcome {
        SavePromptOutcome::Choice {
            choice: SaveChoice::from_id(answer),
            selected: answer,
        }
    }
    /// A failed operation, with the same wording as on Windows.
    pub fn operation_failed(details: &str) -> Self {
        Self {
            style: PromptStyle::Critical,
            message: "The operation could not finish. Your documents remain open.".into(),
            detail: format!("{details}\n\nFile commands remain available to save your work."),
            buttons: vec![(CANCEL_ID, "OK".into())],
        }
    }

    pub fn plan(&self) -> AlertPlan {
        AlertPlan {
            style: self.style,
            message: self.message.clone(),
            detail: self.detail.clone(),
            buttons: self
                .buttons
                .iter()
                .enumerate()
                .map(|(index, (id, title))| AlertButton {
                    title: title.clone(),
                    id: *id,
                    // The first button keeps Return. Setting these explicitly
                    // keeps them when the titles are localized.
                    key: match *id {
                        _ if index == 0 => None,
                        CANCEL_ID => Some(KeyEquivalent::new("\u{1b}", ModifierMask::NONE)),
                        DONT_SAVE_ID => Some(KeyEquivalent::new("d", ModifierMask::COMMAND)),
                        _ => None,
                    },
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(plan: &AlertPlan) -> Vec<&str> {
        plan.buttons.iter().map(|button| button.title.as_str()).collect()
    }

    #[test]
    fn task_dialogs_put_the_default_first_and_cancel_last() {
        let prompt = InAppPrompt::task(
            "Replace the existing file?",
            "/tmp/notes.txt",
            &[(1201, "&Keep Both"), (1202, "&Replace")],
            1202,
        );
        let plan = prompt.plan();
        assert_eq!(titles(&plan), ["Replace", "Keep Both", "Cancel"]);
        assert_eq!(plan.answer(FIRST_BUTTON_RETURN), 1202);
        assert_eq!(plan.answer(FIRST_BUTTON_RETURN + 1), 1201);
        assert_eq!(plan.answer(FIRST_BUTTON_RETURN + 2), CANCEL_ID);
        // Anything else (an aborted modal session, a stray code) is Cancel.
        for response in [0, -1000, 999, FIRST_BUTTON_RETURN + 3] {
            assert_eq!(plan.answer(response), CANCEL_ID, "{response}");
        }
        assert_eq!(
            plan.buttons[2].key,
            Some(KeyEquivalent::new("\u{1b}", ModifierMask::NONE))
        );
    }

    #[test]
    fn destructive_confirmations_default_to_cancel() {
        let prompt = InAppPrompt::confirm("Discard unsaved changes?", "", "Discard");
        let plan = prompt.plan();
        assert_eq!(titles(&plan), ["Cancel", "Discard"]);
        assert_eq!(prompt.style, PromptStyle::Warning);
        assert!(!InAppPrompt::confirmed(plan.answer(FIRST_BUTTON_RETURN)));
        assert!(InAppPrompt::confirmed(plan.answer(FIRST_BUTTON_RETURN + 1)));
        // Cancel is the default here, so it keeps Return rather than Escape.
        assert_eq!(plan.buttons[0].key, None);
    }

    #[test]
    fn save_prompts_follow_the_mac_layout_and_shell_outcomes() {
        let plan = InAppPrompt::save_document("notes.txt \u{2022}").plan();
        assert_eq!(plan.message, "Save changes to notes.txt?");
        assert_eq!(titles(&plan), ["Save", "Don't Save", "Cancel"]);
        assert_eq!(
            plan.buttons[1].key,
            Some(KeyEquivalent::new("d", ModifierMask::COMMAND))
        );
        let outcome = |response| InAppPrompt::save_outcome(plan.answer(response));
        assert_eq!(
            outcome(FIRST_BUTTON_RETURN),
            SavePromptOutcome::Choice {
                choice: SaveChoice::Save,
                selected: SAVE_ID
            }
        );
        assert_eq!(
            outcome(FIRST_BUTTON_RETURN + 1),
            SavePromptOutcome::Choice {
                choice: SaveChoice::DontSave,
                selected: DONT_SAVE_ID
            }
        );
        assert!(matches!(
            outcome(FIRST_BUTTON_RETURN + 2),
            SavePromptOutcome::Choice {
                choice: SaveChoice::Cancel,
                ..
            }
        ));
    }

    #[test]
    fn exit_prompt_lists_documents_and_summarizes_the_rest() {
        let names: Vec<String> = (1..=17).map(|n| format!("note{n}.txt*")).collect();
        let prompt = InAppPrompt::save_all(&names);
        assert_eq!(prompt.message, "Save changes to 17 documents?");
        assert!(prompt.detail.contains("\u{2022} note15.txt\n\u{2022} and 2 more"));
        assert!(!prompt.detail.contains("note16"));
        assert_eq!(prompt.buttons[0], (SAVE_ID, "Save All".into()));
        assert_eq!(
            InAppPrompt::save_all(&["only.txt".into()]).message,
            "Save changes to only.txt?"
        );
        assert_eq!(display_title("  * "), "this document");
    }

    #[test]
    fn about_box_validates_metadata_and_returns_actions() {
        assert!(InAppPrompt::about("bad\0metadata").is_err());
        assert!(InAppPrompt::about(&"x".repeat(MAX_ABOUT_BYTES + 1)).is_err());
        let prompt = InAppPrompt::about("Version 0.2.0").unwrap();
        let plan = prompt.plan();
        assert!(plan.detail.starts_with("Version 0.2.0\n\n"));
        assert_eq!(plan.buttons[0].title, "Close");
        let actions: Vec<_> = (0..4)
            .map(|index| InAppPrompt::about_action(plan.answer(FIRST_BUTTON_RETURN + index)))
            .collect();
        assert_eq!(
            actions,
            [
                None,
                Some(AboutAction::License),
                Some(AboutAction::ThirdPartyNotices),
                Some(AboutAction::CopyDiagnostics)
            ]
        );
    }

    #[test]
    fn a_single_button_alert_keeps_return_on_its_button() {
        let plan = InAppPrompt::operation_failed("Disk full").plan();
        assert_eq!(titles(&plan), ["OK"]);
        assert_eq!(plan.buttons[0].key, None);
        assert_eq!(plan.answer(FIRST_BUTTON_RETURN), CANCEL_ID);
    }
}
