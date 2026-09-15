# QA-FIX-002 — close recovery manifest before sealing

Scope: ISSUE-002 sharing violation; related ISSUE-009 missing complete recovery baseline. Authority: PR-004 Crash recovery requires a sealed baseline before Complete recovery. Static diagnosis found retain_recovery keeps a writable source.json handle across open_retained, whose Windows read seal denies writable handles.

Implementation: release the durable manifest writer before opening the retained store with the existing strict read seal. No sharing-policy relaxation or historical-record rewrite. Existing failed captures cannot be repaired by this change.

Verification: static handle-lifetime review only; builds/tests and fresh-process recovery verification deferred by user until the fix queue lands. ISSUE-009 remains pending verification on newly generated recovery data.

Batch fixture cleanup correction: paged spill behavior assertions passed, then Windows removal failed. Added an I/O-worker completion barrier after dropping the view/snapshot so previously published worker results cannot retain actors during fixture deletion. Production sharing unchanged; parent owns validation.
