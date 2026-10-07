// SPDX-License-Identifier: MPL-2.0
//! A question the shell draws as its own modal (`shell::prompt`): its words and
//! buttons. Only a system whose prompts cannot be native modal dialogs (Linux,
//! where the shell draws them while its event loop keeps running) produces
//! one; on Windows and macOS every prompt is a native dialog, so the seam never
//! returns a view there and these types stay unused.
#![cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "only Linux draws prompts in the shell")
)]

/// How serious the question is; it picks the accent colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptLevel {
    Information,
    Question,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptButtonView {
    /// The answer this button gives (the Windows dialog's button id).
    pub id: i32,
    pub label: String,
    /// The letter that presses the button, as the underlined letter of a
    /// Windows dialog button does.
    pub access_key: Option<char>,
}

/// One question, in display order. Escape answers `cancel_id`; Enter answers
/// the focused button, which starts at `default_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptView {
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub footer: Option<String>,
    pub level: PromptLevel,
    pub buttons: Vec<PromptButtonView>,
    pub default_id: i32,
    pub cancel_id: i32,
}
