# QA-FIX-015 — bounded distant caret preparation

Authority: PR-003/019 bounded viewport work. Unwrapped long-line caret navigation previously shaped every 4KiB prefix fragment before reaching a distant End. Re-anchor an unwrapped fragment near the requested caret using existing local horizontal positioning; retain progressive wrapped layout. Report a bounded-column limitation explicitly rather than permanent indexing when no column worker exists.

Verification: static only; builds/tests deferred. Native 20MiB-line End, backward navigation and exact wrapped layout pending.

Near-fragment backward caret navigation reanchors when caret crosses the fragment start; Home reanchors to line start. Exact horizontal distance across skipped prefixes remains unavailable and requires native acceptance; no global x-width or full column-index claim is made.

Exact-column follow-up: shared bounded column worker now streams 4KiB grapheme chunks with Unicode pre-context and cancellation, caching at most512 scalar-aligned grapheme checkpoints. UI/accessibility publish counting only while a real job is pending, then exact column; source errors remain explicit. Unwrapped leftward panning at a reanchored origin loads an earlier overlapping fragment and restores the old origin's measured position before applying the delta; repeated pan/Left can reach the prefix. Wrap semantics remain unchanged. Parent validation pending; no tests/builds here.
