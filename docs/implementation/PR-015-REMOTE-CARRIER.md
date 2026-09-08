# PR-015 action-scoped remote read carrier

Source implementation in closure-final; automated validation and real remote acceptance remain pending the coordinator batch. No network request or native QA was performed for this change.

RemoteReadGrant is an in-memory, exact-path/action, single-admission capability with bounded admission expiry and explicit revocation. Open and Reload flow through the existing file worker using its scoped filesystem; Follow retains one admitted capability across bounded reads, source rotation reload and snapshot capture. Session paths do not recreate grants. Windows rejects malformed paths before access, pins no-follow ancestor handles, checks reparse/offline attributes, and permits only the requested remote read. Ordinary filesystem policy still rejects UNC and mapped remote access before descendant metadata queries. Cache writes remain local and remote writes are never authorized by this grant.

Open/Reload use the paged transcode lane. A paused conversion retains the admitted capability and reload target; Resume does not claim again. Cancellation stops the current worker request; grant revocation is checked during conversion steps, including resumed work. Source guards are released after private transcode sealing or cancellation cleanup. Follow uses sharing suitable for append/rotation while retaining ancestor guards.

Reload replaces the captured tab only if its identity/revision remain unchanged, preserves user read-only policy, and refuses unconfirmed dirty content. Paged reload stays paged. Native session fallback captures authoritative global selection endpoints.

Queued regressions cover exact path/action, single admission, expiry, revocation, ordinary-service default denial after a scoped read, malformed remote lexical rejection without network I/O, and readonly paged reload replacing the same tab while preserving policy. These tests are source additions, not executed evidence.
