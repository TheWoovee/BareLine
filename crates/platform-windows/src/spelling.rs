// SPDX-License-Identifier: MPL-2.0
//! Windows Spell Checking API adapter (BIZ-31; Windows 8 and later). Every COM
//! object is created, used and released on the spelling worker thread that
//! called the factory, inside that thread's own multithreaded apartment.
use bareline_platform::spelling::{SpellChecker, SpellCheckerFactory};
use std::{marker::PhantomData, sync::Arc};
use windows::{
    Win32::{
        Foundation::S_OK,
        Globalization::{GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory, ISpellingError},
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize, IEnumString,
        },
    },
    core::{PCWSTR, PWSTR},
};

/// A factory for the spelling worker: each call builds a checker for the
/// user's Windows locale, falling back to US English.
pub fn spell_checker_factory() -> SpellCheckerFactory {
    Arc::new(|| WindowsSpellChecker::new().map(|checker| Box::new(checker) as Box<dyn SpellChecker>))
}

/// Balances a successful `CoInitializeEx` on the thread that made it.
struct Apartment(PhantomData<*const ()>);
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: created only after CoInitializeEx succeeded on this thread
        // (the marker keeps it there); the checker is released before this runs.
        unsafe { CoUninitialize() };
    }
}

pub struct WindowsSpellChecker {
    // Declaration order is drop order: the checker is released before the
    // apartment it was created in closes.
    checker: ISpellChecker,
    _apartment: Option<Apartment>,
}

impl WindowsSpellChecker {
    pub fn new() -> Result<Self, String> {
        // SAFETY: the spelling worker owns this thread; a successful call is
        // balanced by `Apartment` after every COM object here is released.
        let apartment = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .is_ok()
            .then(|| Apartment(PhantomData));
        let unavailable = |error: windows::core::Error| format!("Spell checking is unavailable: {}", error.message());
        // SAFETY: plain COM calls with NUL-terminated tags that outlive each call.
        unsafe {
            let factory: ISpellCheckerFactory = CoCreateInstance(
                &windows::Win32::Globalization::SpellCheckerFactory,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(unavailable)?;
            let mut chosen = None;
            for tag in [user_language(), "en-US".to_owned()] {
                let tag = wide(&tag)?;
                if factory
                    .IsSupported(PCWSTR(tag.as_ptr()))
                    .map_err(unavailable)?
                    .as_bool()
                {
                    chosen = Some(tag);
                    break;
                }
            }
            let Some(tag) = chosen else {
                return Err(
                    "Spell checking is unavailable: Windows has no spelling dictionary for your language.".into(),
                );
            };
            let checker = factory.CreateSpellChecker(PCWSTR(tag.as_ptr())).map_err(unavailable)?;
            Ok(Self {
                checker,
                _apartment: apartment,
            })
        }
    }
}

impl SpellChecker for WindowsSpellChecker {
    fn is_correct(&mut self, word: &str) -> Result<bool, String> {
        let word = wide(word)?;
        // SAFETY: `word` is NUL-terminated and outlives the call.
        unsafe {
            let errors = self
                .checker
                .Check(PCWSTR(word.as_ptr()))
                .map_err(|error| error.message())?;
            // S_OK yields one spelling error; S_FALSE means there are none.
            let mut error: Option<ISpellingError> = None;
            Ok(!(errors.Next(&mut error) == S_OK && error.is_some()))
        }
    }
    fn suggestions(&mut self, word: &str, limit: usize) -> Result<Vec<String>, String> {
        let wide_word = wide(word)?;
        let mut out = Vec::new();
        // SAFETY: `wide_word` outlives the call; each returned string is
        // CoTaskMem-allocated and freed right after it is copied.
        unsafe {
            let list: IEnumString = self
                .checker
                .Suggest(PCWSTR(wide_word.as_ptr()))
                .map_err(|error| error.message())?;
            while out.len() < limit {
                let mut item = [PWSTR::null()];
                let mut fetched = 0u32;
                if list.Next(&mut item, Some(&mut fetched as *mut u32)) != S_OK || fetched == 0 || item[0].is_null() {
                    break;
                }
                let text = item[0].to_string();
                CoTaskMemFree(Some(item[0].0.cast()));
                // A correctly spelled word lists itself; that is no suggestion.
                if let Ok(text) = text
                    && !text.is_empty()
                    && text != word
                {
                    out.push(text);
                }
            }
        }
        Ok(out)
    }
    fn add_to_dictionary(&mut self, word: &str) -> Result<(), String> {
        let word = wide(word)?;
        // SAFETY: `word` is NUL-terminated and outlives the call.
        unsafe { self.checker.Add(PCWSTR(word.as_ptr())) }.map_err(|error| error.message())
    }
}

/// The user's Windows locale name (for example "en-GB"), or US English.
fn user_language() -> String {
    // LOCALE_NAME_MAX_LENGTH, including the terminating NUL.
    let mut buffer = [0u16; 85];
    // SAFETY: the buffer is writable for its whole length.
    let length = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if length > 1 {
        String::from_utf16_lossy(&buffer[..length as usize - 1])
    } else {
        "en-US".into()
    }
}

fn wide(text: &str) -> Result<Vec<u16>, String> {
    if text.contains('\0') {
        return Err("Words cannot contain NUL characters".into());
    }
    Ok(text.encode_utf16().chain(Some(0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "needs a Windows spelling dictionary for en-US or the user's locale, which CI images may lack"]
    fn windows_dictionary_flags_a_misspelling_and_suggests_the_word() {
        let mut checker = (spell_checker_factory())().expect("spell checker");
        assert!(checker.is_correct("house").unwrap());
        assert!(!checker.is_correct("hoouse").unwrap());
        let suggestions = checker.suggestions("hoouse", 5).unwrap();
        assert!(suggestions.len() <= 5);
        // Add to Dictionary is not exercised: it writes the user's real word list.
        assert!(suggestions.iter().any(|word| word == "house"), "{suggestions:?}");
    }
    #[test]
    fn words_with_nul_are_rejected_before_reaching_com() {
        assert!(wide("a\0b").is_err());
        assert_eq!(wide("ab").unwrap(), vec![u16::from(b'a'), u16::from(b'b'), 0]);
    }
}
