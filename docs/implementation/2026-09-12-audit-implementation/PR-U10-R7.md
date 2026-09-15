# PR-U10-R7 — Retire verified stale external-change warnings

Baseline: `64e27f10300a798616dbf388b5a05e0eac1ff209`.

## Observed behavior

The native audit saved a 29-byte resident document with SHA-256 `A3990C6DD24462834E429057064471A74B5932D988B9D6C4329EE8D88F280609`, closed its clean clone views, reopened the same path with matching UIA and disk bytes, then saw the dirty-document external-change warning without another write. The warning text is emitted only while an editor is dirty. Retained evidence is `target/qualification/final-native-audit/evidence/r6-resident-false-external-alert-uia.json`, the adjacent `r6-resident-false-external-alert.jpg` screenshot, and `r6-resident-false-external-alert-file.json`. The screenshot also shows that the extended path consumed both visible toast lines, hiding the external-change meaning.

## Cause

Watcher conflicts publish a persistent typed notification keyed by the lossless path. A later identity check returning unchanged removed the internal conflict set entry but did not resolve that notification. Persistent operation ownership correctly let the warning survive document closure; after a clean reopen verified the current disk identity, the already-resolved warning could therefore remain visible. The evidence supports stale notification lifecycle as the cause, rather than a fresh identity mismatch after reopen.

## Correction

Use one lossless serialized path-derived notification identity for publication and resolution. A matching asynchronous identity check or explicit Keep Current Buffer action now resolves both the watcher conflict and its typed notification. Scheduling automatic reload is not treated as success; the warning remains until a later matching check verifies the resulting disk identity. Changed/error results still publish a persistent warning, dirty bytes remain untouched, and stale checks whose expected identity is missing or no longer matches the current document continue to request a fresh check.

The visible warning now leads with the external-change or unavailable-file meaning and the concise filename. Its complete extended display path and all recovery actions remain in notification details.

## Contained evidence plan

A headless regression creates a dirty-period warning, performs a real save, closes the clean document, reopens the same canonical path, and applies the matching identity result. It asserts exact disk bytes, clean reopened state, visible warning meaning and complete path details, persistence across close and pending automatic reload, rejection of a stale pre-save result, then terminal removal only after verified equality. Root owns rebuilt native qualification and the contained command `cargo test -p bareline --bin bareline windows_app::watch::tests::verified_reopen_retires_a_persistent_dirty_period_conflict --offline --locked`.
