# DEV-003 — Adapter foundation and plain-text procedure

## Result

**20 focused source tests pass.** The explicitly configured LF native control
passes all three product steps: Unicode/line break input, exact Save/Close/Open
round trip, and edit/Undo/Redo. Its editor exits cleanly with code zero; the driver,
adapter and outer runner also exit zero. It observes selected-tab state rather
than assuming Close leaves no empty placeholder tab.

**The CRLF cell fails.** Native Enter inserts LF despite a CRLF new-file setting;
an earlier captured attempt also reached Save As and recorded LF on disk. The
final strict CRLF run fails s1 and marks dependent s2/s3 NOT_RUN. No expected text
or byte stream is normalized to obtain a pass. Source inspection finds both
files.default_eol and files.default_encoding unused outside their settings
definition/resolution. Non-UTF-8 defaults still require a native capture. The
defect is NEW-FILE-DEFAULTS under QUAL-012/parity-038, related to QUAL-007.

DEV-003 is **in progress**: the other twelve product procedures are unimplemented.
Their explicit NOT_RUN responses never pass the runner. 47 of 49 backlog items
remain open; DEV-001 and DEV-002 are implemented with focused verification and
retain their final qualification gates.

## Final captures

| Capture | Result | Scope |
|---|---|---|
| [Unit receipt](captures/adapter-unit-tests-final.json) | 20/20; 0.212 s | Request/path/hash guards, unsupported cells, source drift, process exits/timeouts, artifact tampering, atomic publication and LF/CRLF fixture distinction. No editor launched. |
| [LF control receipt](captures/plain-text-lf-control5.json) | PASS; s1/s2/s3 PASS | Real native workflow, owned scratch/profile, exact UTF-8 LF file bytes, clean editor Exit. |
| [CRLF regression receipt](captures/plain-text-crlf-regression-final.json) | FAIL; s1 FAIL, s2/s3 NOT_RUN | Confirmed ignored CRLF default, retained as an open defect. |
| [Earlier CRLF disk observation](runs/plain-text-attempt8/scratch/native-observations.json) | FAIL | Exact saved LF bytes differ from the independently expected CRLF bytes. |

The final LF run ID is `3368329f-b61a-4338-886c-e06e1221eeae`; the final CRLF run
ID is `e63f66a3-9969-4558-9416-76d1ebddff62`. Both use the existing DEV-002 debug
binary SHA-256 `b52da9546ac1547a75a9679c8afa4d0c70c23fd4c23b57219112a58b54aff414`.
The captured host is Windows 10.0.26200, software renderer, dark configured
profile, keyboard automation and observed 96-DPI window (100%). This does not
establish other OS, renderer, theme, monitor, physical IME or screen-reader cells.
The reviewer field identifies a development capture; independent review remains
pending. No reviewed plain_text-to-AC mapping was added or acceptance imported.

## Retention and failures

[verification-summary.json](verification-summary.json) binds the final source
files and binary, lists all 14 native attempts and records the implemented subset.
[retained-files.json](retained-files.json) maps 184 exact-byte retained artifacts
to their original paths and SHA-256 hashes (about 635 KB total). Copies under
captures/ and runs/ preserve the original records; source/ contains inert copies
of the final adapter, driver, unit tests and manifest. Receipt commands and
stdout/stderr bindings remain unchanged. Native response artifact references
point to original scratch paths; use the retention mapping when reviewing copies.

Failed attempts retain initial focus/exit observation faults, the PowerShell
UInt16 alias correction, process-zero decoration handling, an incorrect initial
newline assumption, modern/legacy filename control discovery, async file-read
sharing, selected-tab versus total-tab assumptions, and the foreground stop.
They were corrected in the adapter, not promoted as product passes. A fresh
scratch run followed each correction. Each action was submitted once per run;
observer polling did not replay actions after failed assertions.

All three final receipts record stable source during capture and verified raw
stdout/stderr hashes. The LF control records clean editor Exit. Failed attempts
terminate only the created editor, and the nested/outer kill-on-close Jobs own
descendants. A final process check found no remaining bareline processes.

**No full suite, aggregate native journey run, or application build ran.**
Later documentation/ledger updates do not relabel these records as a new
application candidate or final release acceptance.
