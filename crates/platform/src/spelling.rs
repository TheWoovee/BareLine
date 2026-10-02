// SPDX-License-Identifier: MPL-2.0
//! Portable spell-checker contract (BIZ-31). The Windows adapter wraps the
//! Spell Checking API; neutral crates see only this trait. A checker is built
//! and used on the one worker thread that owns it, never on the UI thread.
use std::sync::Arc;

/// Suggestions offered for one word, the most likely first.
pub const MAX_SUGGESTIONS: usize = 5;

pub trait SpellChecker {
    /// Whether the dictionary accepts `word`. `word` is one token without spaces.
    fn is_correct(&mut self, word: &str) -> Result<bool, String>;
    /// At most `limit` replacements for `word`, the most likely first.
    fn suggestions(&mut self, word: &str, limit: usize) -> Result<Vec<String>, String>;
    /// Add `word` to the user's dictionary so every later check accepts it.
    fn add_to_dictionary(&mut self, word: &str) -> Result<(), String>;
}

/// Builds a checker on the worker thread that will own and release it, so an
/// adapter can set up per-thread state (a COM apartment) first. `Err` names why
/// spell checking is unavailable in words for a person.
pub type SpellCheckerFactory = Arc<dyn Fn() -> Result<Box<dyn SpellChecker>, String> + Send + Sync>;
