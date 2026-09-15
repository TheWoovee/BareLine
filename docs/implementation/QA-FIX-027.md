# QA-FIX-027 — schedule macro replay ticks

Authority: PR-014 acknowledgement-driven playback. Play requested only a redraw; replay tick ran only on user events. Its WaitUntil was then overwritten by the shell's caret/idle deadline, so playback could remain Running indefinitely. Notify after Play, retain the next macro/process polling deadline in runtime state, pump only when due in about_to_wait, and merge that deadline with other shell deadlines.

Verification: static scheduling review only; builds/tests deferred. Replay acknowledgements unchanged; no unconditional busy timer. Native record/play completion pending.
