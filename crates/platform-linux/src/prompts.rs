// SPDX-License-Identifier: MPL-2.0
//! The About window and the product's message prompts as data. Linux has no
//! native message box every desktop provides (and spawning `zenity` would put a
//! foreign window and process into the editor), so each prompt is described as
//! an [`InAppPrompt`] the shell draws as its own modal. The wording, buttons and
//! defaults are the Windows adapter's, so both systems ask the same questions;
//! the answer helpers map a pressed button back to the same results.
use std::path::Path;

/// The Windows `IDCANCEL` answer: Escape, the close button or Cancel.
pub const CANCEL_ID: i32 = 2;
const SAVE_ID: i32 = 1101;
const DONT_SAVE_ID: i32 = 1102;
const OVERWRITE_ID: i32 = 1103;
const YES_ID: i32 = 6;
const NO_ID: i32 = 7;
const OK_ID: i32 = 1;
const LICENSE_ID: i32 = 1001;
const NOTICES_ID: i32 = 1002;
const DIAGNOSTICS_ID: i32 = 1003;
/// Names listed in the exit prompt before the rest are summarized, so the
/// buttons stay on screen however many documents are unsaved (UI-18).
const SAVE_ALL_LISTED: usize = 15;

/// Answer to a "save changes?" prompt. `Cancel` also covers Escape and closing
/// the prompt, so callers can treat it as "do nothing".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePromptOutcome {
    Choice { choice: SaveChoice, selected: i32 },
    Failure(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutAction {
    License,
    ThirdPartyNotices,
    CopyDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptSeverity {
    Information,
    Question,
    Warning,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptButton {
    pub id: i32,
    /// The label without its access-key marker.
    pub label: String,
    /// The underlined letter (Windows `&` marker), if any.
    pub access_key: Option<char>,
}
impl PromptButton {
    /// `"Do&n't Save"` becomes the label `Don't Save` with access key `n`.
    pub fn new(id: i32, marked: &str) -> Self {
        let mut label = String::with_capacity(marked.len());
        let mut access_key = None;
        let mut characters = marked.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '&' {
                match characters.next() {
                    Some('&') => label.push('&'),
                    Some(next) => {
                        access_key.get_or_insert(next.to_ascii_lowercase());
                        label.push(next);
                    }
                    None => {}
                }
            } else {
                label.push(character);
            }
        }
        Self { id, label, access_key }
    }
}
/// One modal the shell draws. Buttons are in display order; Escape and closing
/// the prompt answer `cancel_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InAppPrompt {
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub footer: Option<String>,
    pub severity: PromptSeverity,
    pub buttons: Vec<PromptButton>,
    pub default_id: i32,
    pub cancel_id: i32,
}
/// Tab titles carry the unsaved marker; prose in a prompt must not repeat it.
fn display_title(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches(['\u{2022}', '*', '\u{25cf}']).trim();
    if trimmed.is_empty() {
        "this document".to_string()
    } else {
        trimmed.to_string()
    }
}
fn yes_no(title: &str, content: String, severity: PromptSeverity) -> InAppPrompt {
    InAppPrompt {
        title: title.into(),
        instruction: String::new(),
        content,
        footer: None,
        severity,
        buttons: vec![PromptButton::new(YES_ID, "&Yes"), PromptButton::new(NO_ID, "&No")],
        // The safe answer is the default (MB_DEFBUTTON2 on Windows).
        default_id: NO_ID,
        cancel_id: NO_ID,
    }
}
impl InAppPrompt {
    /// The shared modal for the product's own decisions. A Cancel button is
    /// always added, so Escape returns [`CANCEL_ID`].
    pub fn task(title: &str, instruction: &str, content: &str, buttons: &[(i32, &str)], default_id: i32) -> Self {
        let mut list: Vec<PromptButton> = buttons
            .iter()
            .map(|(id, label)| PromptButton::new(*id, label))
            .collect();
        list.push(PromptButton::new(CANCEL_ID, "Cancel"));
        Self {
            title: title.into(),
            instruction: instruction.into(),
            content: content.into(),
            footer: None,
            severity: PromptSeverity::Question,
            buttons: list,
            default_id,
            cancel_id: CANCEL_ID,
        }
    }
    /// Metadata supplied by the application, never document contents. Refuses
    /// oversized or NUL-carrying details, as on Windows.
    pub fn about(details: &str) -> Result<Self, String> {
        if details.len() > 16 * 1024 || details.contains('\0') {
            return Err("Invalid About metadata".into());
        }
        Ok(Self {
            title: "About Bareline".into(),
            instruction: "Bareline".into(),
            content: details.into(),
            footer: Some(
                "Plain text. Full power. No weight.\nCore: MPL-2.0 \u{b7} Extension SDK: MIT OR Apache-2.0\nPrivacy policy: https://github.com/TheWoovee/BareLine/blob/master/PRIVACY.md".into(),
            ),
            severity: PromptSeverity::Information,
            buttons: vec![
                PromptButton::new(LICENSE_ID, "License"),
                PromptButton::new(NOTICES_ID, "Third-party notices"),
                PromptButton::new(DIAGNOSTICS_ID, "Copy diagnostics"),
                PromptButton::new(CANCEL_ID, "Close"),
            ],
            default_id: CANCEL_ID,
            cancel_id: CANCEL_ID,
        })
    }
    /// Save / Don't Save / Cancel for one document that is about to close.
    pub fn save_document(name: &str) -> Self {
        Self::task(
            "Bareline",
            &format!("Save changes to {}?", display_title(name)),
            "Your changes will be lost if you don't save them.",
            &[(SAVE_ID, "&Save"), (DONT_SAVE_ID, "Do&n't Save")],
            SAVE_ID,
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
        Self::task(
            "Bareline",
            &format!("Save changes to {} documents?", names.len()),
            &format!(
                "These documents have unsaved changes:\n\n{list}\n\nYour changes will be lost if you don't save them."
            ),
            &[(SAVE_ID, "Save &All"), (DONT_SAVE_ID, "Do&n't Save")],
            SAVE_ID,
        )
    }
    pub fn discard_and_reload(name: &str) -> Self {
        yes_no(
            "Bareline",
            format!("Discard unsaved changes to {name} and reload it from disk?"),
            PromptSeverity::Warning,
        )
    }
    pub fn stop_monitoring() -> Self {
        yes_no(
            "Unlock monitored file",
            "Stop monitoring and load the current file for editing?\n\nThe tab will remain open.".into(),
            PromptSeverity::Question,
        )
    }
    /// Reinterpretation reloads original bytes and therefore discards unsaved edits.
    pub fn encoding_reinterpret(name: &str, target: &str) -> Self {
        let display = |value: &str| value.chars().filter(|c| !c.is_control()).take(256).collect::<String>();
        yes_no(
            "Reopen with encoding",
            format!(
                "Reopen {} using {}?\n\nUnsaved changes in this document will be discarded. The file on disk will not be changed.\n\nChoose No to keep the current document and its edits.",
                display(name),
                display(target)
            ),
            PromptSeverity::Warning,
        )
    }
    pub fn overwrite(path: &Path) -> Self {
        Self::task(
            "Bareline",
            "Replace the existing file with this document?",
            &format!(
                "{}\n\nReplacing it will overwrite its current contents.",
                path.display()
            ),
            &[(OVERWRITE_ID, "&Replace")],
            CANCEL_ID,
        )
    }
    pub fn operation_failed(details: &str) -> Self {
        Self {
            title: "Bareline".into(),
            instruction: String::new(),
            content: format!(
                "The operation could not finish. Your documents remain open.\n\n{details}\n\nFile commands remain available to save your work."
            ),
            footer: None,
            severity: PromptSeverity::Error,
            buttons: vec![PromptButton::new(OK_ID, "OK")],
            default_id: OK_ID,
            cancel_id: OK_ID,
        }
    }
}
/// What the About window's pressed button asks for.
pub fn about_action(selected: i32) -> Option<AboutAction> {
    match selected {
        LICENSE_ID => Some(AboutAction::License),
        NOTICES_ID => Some(AboutAction::ThirdPartyNotices),
        DIAGNOSTICS_ID => Some(AboutAction::CopyDiagnostics),
        _ => None,
    }
}
/// The outcome of a save prompt from its pressed button.
pub fn save_outcome(selected: i32) -> SavePromptOutcome {
    SavePromptOutcome::Choice {
        choice: SaveChoice::from_id(selected),
        selected,
    }
}
/// Whether a yes/no prompt was answered yes.
pub fn confirmed(selected: i32) -> bool {
    selected == YES_ID
}
/// Whether the overwrite prompt was answered Replace.
pub fn overwrite_confirmed(selected: i32) -> bool {
    selected == OVERWRITE_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_keys_are_split_from_labels() {
        let button = PromptButton::new(1, "Do&n't Save");
        assert_eq!((button.label.as_str(), button.access_key), ("Don't Save", Some('n')));
        let plain = PromptButton::new(2, "Cancel");
        assert_eq!((plain.label.as_str(), plain.access_key), ("Cancel", None));
        assert_eq!(PromptButton::new(3, "R&&D").label, "R&D");
    }
    #[test]
    fn save_prompts_match_the_windows_wording_and_answers() {
        let single = InAppPrompt::save_all(&["notes.txt \u{2022}".to_owned()]);
        assert_eq!(single.instruction, "Save changes to notes.txt?");
        assert_eq!(single.buttons[0].label, "Save");
        assert_eq!(single.default_id, SAVE_ID);
        assert_eq!(single.buttons.last().unwrap().id, CANCEL_ID);
        let names: Vec<String> = (1..=40).map(|n| format!("Untitled {n}")).collect();
        let many = InAppPrompt::save_all(&names);
        assert_eq!(many.buttons[0].label, "Save All");
        assert_eq!(many.instruction, "Save changes to 40 documents?");
        assert!(many.content.contains("Untitled 15\n"));
        assert!(!many.content.contains("Untitled 16"));
        assert!(many.content.contains("and 25 more"));
        assert_eq!(
            save_outcome(SAVE_ID),
            SavePromptOutcome::Choice {
                choice: SaveChoice::Save,
                selected: SAVE_ID
            }
        );
        assert_eq!(SaveChoice::from_id(DONT_SAVE_ID), SaveChoice::DontSave);
        assert_eq!(SaveChoice::from_id(CANCEL_ID), SaveChoice::Cancel);
        assert_eq!(
            InAppPrompt::save_document("  ").instruction,
            "Save changes to this document?"
        );
    }
    #[test]
    fn questions_default_to_their_safe_answer() {
        for prompt in [
            InAppPrompt::discard_and_reload("a.txt"),
            InAppPrompt::stop_monitoring(),
            InAppPrompt::encoding_reinterpret("a.txt\u{7}", "UTF-16"),
        ] {
            assert_eq!(prompt.default_id, NO_ID);
            assert_eq!(prompt.cancel_id, NO_ID);
            assert!(!confirmed(prompt.default_id));
            assert!(!prompt.content.contains('\u{7}'));
        }
        assert!(confirmed(YES_ID));
        let overwrite = InAppPrompt::overwrite(Path::new("/tmp/kept.txt"));
        assert_eq!(overwrite.default_id, CANCEL_ID);
        assert!(!overwrite_confirmed(overwrite.default_id));
        assert!(overwrite_confirmed(OVERWRITE_ID));
        assert!(InAppPrompt::operation_failed("disk full").content.contains("disk full"));
    }
    #[test]
    fn about_answers_actions_and_refuses_bad_metadata() {
        let about = InAppPrompt::about("Version 0.2.0").unwrap();
        assert_eq!(about.title, "About Bareline");
        assert_eq!(about.buttons.len(), 4);
        assert_eq!(about_action(about.buttons[0].id), Some(AboutAction::License));
        assert_eq!(about_action(about.buttons[1].id), Some(AboutAction::ThirdPartyNotices));
        assert_eq!(about_action(about.buttons[2].id), Some(AboutAction::CopyDiagnostics));
        assert_eq!(about_action(about.cancel_id), None);
        assert!(InAppPrompt::about("a\0b").is_err());
        assert!(InAppPrompt::about(&"x".repeat(16 * 1024 + 1)).is_err());
    }
}
