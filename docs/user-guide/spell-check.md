# Spell check

Bareline underlines misspelled words using the spelling dictionaries built into Windows. Spell check is an experimental feature in this preview.

## What is checked

- **Plain text and Markdown** are checked when the **Spell check** setting (`editor.spell_check`, under **Settings > Editor**) is on, which is the default.
- **Code** is not checked unless its language turns it on under **Language behavior** (`language.policies`), for example `rust.spell_check = true`. Then only comments and strings are checked.
- Only the visible text, plus a small margin around it, is checked, on a background thread; the rest is checked as you scroll.
- Not checked yet: documents in [large-file mode](large-files.md) and the second pane of a split view.

## Fixing a word

Right-click an underlined word, or place the caret on it and open **Edit > Spelling**:

| Command | Effect |
| --- | --- |
| Up to five suggestions | Replace the word with the suggestion, as one undoable edit. |
| **Add to Dictionary** | Adds the word to your Windows user dictionary, which other Windows applications that use the Windows spelling dictionaries share. |
| **Ignore All** | Accepts the word everywhere until Bareline exits. |
| **Spell Check** | Turns spell checking on or off (the same as the setting). |

## Dictionaries and languages

Bareline asks Windows for a spelling dictionary for your Windows locale (for example `en-GB`), and uses US English if Windows has none for it. If Windows has neither, spell checking reports that it is unavailable. To check another language, add its language pack and spelling dictionary in Windows Settings. Bareline does not ship dictionaries of its own and does not send words anywhere.

## Accessibility

Suggestions are ordinary menu items that screen readers announce. Misspellings themselves are not yet reported to screen readers as text attributes.
