# QA-FIX-020 — reject unknown logical shortcut keys

Authority: PR-011 keymap validation. KeyChord::parse previously accepted every nonempty logical key token, including DefinitelyNotAKey. Validate against a single printable Unicode character, supported named editing/navigation keys and F1–F35. Shared parsing covers mapper and imported keymaps; invalid edits preserve the existing binding.

Verification: static only; builds/tests deferred. Physical-key validation is unchanged.
