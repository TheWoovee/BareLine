# Large files

Bareline edits most files entirely in memory (the normal editor). Large files open in **large-file mode** instead, where Bareline reads the file in pages as you move through it and keeps only part of it in memory. Large-file mode is a preview feature; read its known issues in the [README](../../README.md#known-issues-in-this-preview).

## When large-file mode is used

- A file larger than the **Large-file threshold** setting (`document.resident_max_bytes`, 256 MiB by default) opens in large-file mode.
- A smaller file whose text does not fit the normal editor's memory budget falls back to large-file mode when it is opened or reloaded.
- When an open fails, the tab stays with the error and offers **Retry** and **Open Read-Only (Large-File Mode)**.
- **Encoding > Interpret As** on a file that is too large for the normal editor in the chosen encoding reopens it in large-file mode with that encoding.

The threshold and the other limits below are in **Settings > Advanced**. Changes apply to files opened afterwards.

| Setting | Default | Meaning |
| --- | --- | --- |
| Large-file threshold | 256 MiB | Files larger than this open in large-file mode. |
| Large-file block size | 1 MiB | How much of the file is read at a time. |
| Memory kept per large file | 64 MiB | Pages of one large file kept in memory. |
| Document memory limit | 256 MiB | Memory shared by all open documents. |
| Undo memory limit | 128 MiB | Memory shared by the undo history of all open documents. When it is exceeded, the oldest history of any document is dropped first. |
| Undo steps kept | 100,000 | Undo and redo steps per document. |
| Temporary disk space for encoding conversion | 20 GiB | Disk space a conversion to another encoding may use. Free disk space still applies. |

These are resource limits, not a promise that a file of a given size will open or that an operation will finish in a given time.

## Line numbers

A large file opens and scrolls before its lines have been counted. Until a background count reaches the end, line numbers are estimates and the status bar shows **Line numbers estimated · indexing** with the progress. The count continues across edits: an edit keeps the counted lines before it and shifts those after it. If the count fails three times, the status bar shows **Line numbers estimated · count stopped**.

The time the count takes on multi-gigabyte files has not been re-measured since it was rewritten for this preview; see [STATUS.md](../STATUS.md).

## What works differently in large-file mode

| Area | Behavior in large-file mode |
| --- | --- |
| Editing | Typing, deleting, pasting, line operations, multiple carets and rectangular selections work. A single command acts on at most 1,024 selections and 4,096 edits; larger requests are refused with a message. Large deletions and pastes are staged on disk so that their undo text does not fill memory. |
| Column paste | A column paste that cannot be prepared (past the last line, or over lines not yet measured) pastes the text as ordinary text. |
| Search | Literal and Extended searches, and regular expressions without anchors or look-behind, are searched page by page. A regular expression that uses `^`, `$`, `\b`, `\B`, `\A`, `\G`, `\X`, look-behind or backtracking verbs needs the whole text as context, which is limited to 64 MiB (the same limit applies to documents in the normal editor); on larger files it stops and reports incomplete results ("Regex context exceeds 64 MiB"). |
| Replace All | Refuses more than 10,000 replacements before changing anything, and reports how many matches there were. |
| Compare | Large files are aligned on lines that occur once on each side. With very many distinct lines the alignment uses a sample, and the status reads "Coarse comparison (memory limit)". See [Compare](compare.md). |
| Following a log | **Follow New Content** treats growth of the file as appended text. A file that is shortened, replaced or rewritten is reported as changed. |
| Macros | A macro playback is undone in one step, as in the normal editor. |
| Clipboard | Copying is limited by the `clipboard.max_bytes` setting (1 GiB by default). |
| JSON and XML tools | Format at most 1 MiB of selected text. |
| Hex View | Shows the first 1 MiB of the file's original bytes. |
| Not available | Spell check, **Rename**, and the `$(CURRENT_WORD)` Run variable. |

## Saving

Saving a large file writes the whole file. The original bytes of parts you did not edit are copied unchanged. On local NTFS volumes the new file replaces the old one in one step; see the [README](../../README.md#known-issues-in-this-preview) for network and other volumes. Recovery journals protect unsaved edits in large-file mode as in the normal editor ([Recovery](recovery.md)).
