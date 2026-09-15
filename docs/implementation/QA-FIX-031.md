# QA-FIX-031 — recheck watch results after save completion

Authority: file-watch source-change contract. Watch results could compare replaced disk bytes against the pre-save fingerprint while the editor was busy, then retain the resulting conflict forever. Defer busy-document checks and requeue stale expected identities; a fresh unchanged result clears an old conflict. Do not clear unverified external changes on save.

Verification: static race review only; builds/tests deferred. Paged save byte oracle and true external-write detection pending native verification.
