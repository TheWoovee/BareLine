# QA-FIX-017 — seed occurrence selection from caret word

Authority: PR-006 repeated occurrence selection. Ctrl+D returned unchanged selections when primary was empty. Use a bounded Unicode word window to select the caret word on the first invocation; later calls retain existing next-occurrence behavior. Do not scan an unbounded line on the UI thread.

Verification: static only; builds/tests deferred. Ordinary drag and double-click pointer selection remain separate concrete follow-ups.

Pointer follow-up: shared primary/split pointer path tracks plain selection drag, extends with existing shaped hit testing, clears capture on release/focus loss, and detects a bounded double-click to select the caret word. Clicks inside existing selections retain text-transfer handling. Paged Ctrl+D uses a bounded captured-source word window. Native input verification remains pending.
