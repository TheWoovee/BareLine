# QUAL-012 — New-file encoding and EOL defaults

**State:** implemented with focused verification. NEW-FILE-DEFAULTS was found by DEV-003; broader QUAL-012 qualification remains open. Work stayed local with focused checks and one incremental debug application build.

## Contract and scope

Consume files.default_encoding and files.default_eol when creating a new document, including typing into the empty state and the replacement empty tab after Close. Apply opening metadata before actor publication so a new document is clean and has no synthetic Undo entry. Existing documents, raw pasted text and mixed line endings must retain their content/policy. Native Enter and smart indentation must honor the new document's EOL. Resolve the System encoding through the Windows ANSI code page; explicitly refuse unsupported catalog entries rather than silently writing UTF-8.

Authority: [PR-012 settings](../../blueprint/PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md), [PR-007 byte fidelity and EOL](../../blueprint/PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md), and the [DEV-003 retained regression](../../qa/2026-09-15-dev003-product-adapter/README.md). Reuse document metadata, existing encoding save policy and the native product adapter.

## Verification

Six focused Rust checks pass: new clean state, changes affecting only later documents, encoding/BOM serialization, LF/CRLF smart indentation, Undo/Redo and existing mixed-EOL fidelity. Explicit full-document EOL conversion before the first newline updates the policy with one Undo entry. System uses GetACP through the Windows platform layer and refuses unsupported code pages with a visible message. The Close placeholder refreshes defaults and preserves creation errors.

Twenty-two focused adapter checks pass, including binding the selected EOL and encoding/BOM into the expected native fixture. The strict native UTF-8/CRLF regression passes all three steps and checked editor Exit 0. The initial CRLF attempt and additional LF/UTF-8-BOM attempt stopped before typing on lost foreground; both failures are retained. Further native encoding cells were deferred after that recurrence. Byte-level save/reopen tests pass for all four Unicode settings, including UTF-16 LE/BE BOMs and literal mixed EOL under different defaults.

[Retained receipts and limits](../../qa/2026-09-15-new-file-defaults/README.md) bind the successful native run to debug binary c3b618c1dadce5ecd3ed023357bf48aa35d4585f3b057ef8a5e9ea502cd02a4f and the build's stable dirty-source identity. No full workspace suite, aggregate native run or release build ran.

## Completion boundary

The concrete wiring defect is implemented and focused-verified. QUAL-012 still needs broader settings/theme/DPI/locale qualification and the remaining native encoding cells; DEV-003 still needs twelve product procedures. System code pages outside the existing codec catalog remain explicitly unsupported; the host's System option has not been qualified natively. No AC evidence was imported and no capability was declared fully qualified.

**47 of 49 top-level backlog items remain open.** This sub-fix stays inside QUAL-012, so it does not close that parent item. Parity-038 returns to implemented-awaiting-qualification: 40 awaiting qualification, one open defect, seven deferred and four intentional limits. Continue with DEV-003's code_config procedure next; final integrated checks remain deferred to the end at the owner's request.
