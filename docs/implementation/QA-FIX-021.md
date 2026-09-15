# QA-FIX-021 — activate existing workspace file

Authority: UI-ED explorer selection and active tab describe the same document. Explorer Open always queued a new load, whose duplicate detection only reported already open. Check existing workspace paths first and activate the matching tab through the same action as Document List.

Verification: static only; builds/tests deferred. Native workspace activation pending.
