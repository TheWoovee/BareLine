# QA-FIX-032 — native export publication diagnosis

The parent reproduced OS32 in HTML export after the initial batch. Add phase-specific errors and a native publication regression covering a fresh destination and replacement while retaining directory guards. This distinguishes publication from source capture without relaxing filesystem protection. Execution belongs to the parent's focused validation; no tests run here.

Parent regression isolated first-publication failure to commit while the parent directory guard remains held. Directory guards now request FILE_READ_ATTRIBUTES instead of FILE_GENERIC_READ (directory-list access), retaining no-follow root-first validation and identical deny-write/delete sharing. Sealed content readers remain unchanged. The guard additionally verifies the final handle is a directory. Native export regression awaits parent rerun.
