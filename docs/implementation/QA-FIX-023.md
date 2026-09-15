# QA-FIX-023 — completion owns Tab before editor indent

The shared QA-FIX-012 keymap layer now treats language/completion popup as an input owner. Tab reaches language_event's existing acceptance handler instead of the early editor.indent binding. Enter/Tab completion source identity checks are unchanged.

Verification: static only; completion acceptance and native replay deferred to combined verification.
