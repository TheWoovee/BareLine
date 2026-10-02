# Replace in Files

**Search > Workspace Replacement > Replace in Files…** replaces text in files on disk, in a folder you choose; **Replace in Workspace…** does the same for the open workspace folder. Both work in two steps, a preview and an apply, and keep a receipt and backups so that a job can be undone later. Replace in Files is a preview feature; keep independent backups of important folders.

## Preview

The preview lists every file with matches and, under it, each change with the text before and after. Nothing is written yet.

- Choose which changes to apply; **Toggle All Preview Changes** switches all of them.
- **Toggle Preserve Replacement Case** gives each replacement the case of its match when the match is all uppercase, all lowercase or capitalized; a match in mixed case gets the replacement as typed.
- Binary or NUL-heavy files are left out unless you turn on **Toggle Binary Replacement Inclusion**.
- **Toggle Backups for This Replacement Job** turns backups off for this job only. Backups are on by default; without them the job cannot be rolled back.
- Files that cannot be searched, such as locked files, links that lead outside the folder, or files in an unsupported encoding, are listed as skipped; the rest of the folder is still searched.

The search options of the find bar (mode, **Match Case**, **Whole Word**, **. Matches Newline**) apply, and the replacement uses the syntax of the search mode ([Search, replace and regular expressions](search-and-regex.md)).

## Apply

**Apply Reviewed Replacements** writes the chosen changes file by file:

- Before changing a file, Bareline checks that it still has the content the preview was made from; a file that changed since is skipped with "Source changed; review again".
- A file that is open in Bareline is skipped ("File is open; review its document revision"), so an open document and its file never disagree. Replace in the open document instead.
- Read-only files are skipped.
- With backups on, the original of each file is copied to the job's backup folder before the file is replaced.

Cancelling stops before the next file; files already replaced stay replaced and are recorded as such. Exiting Bareline while a job runs asks whether to wait or to cancel and exit.

## Receipts and backups

Each job has a folder named `replace-<process>-<time>-<number>` in the `recovery` folder of your profile ([Recovery](recovery.md#where-journals-live)). It holds the job's receipt, which records the state of every file, and the backups. Apply refuses to run when no durable recovery folder is available; receipts and backups are never kept in `%TEMP%`.

| Command | Effect |
| --- | --- |
| **Restore Last Replacement Backups** | Rolls back the last job of this session, or the job selected in **Manage Replace Backups**: each replaced file whose content is still the job's output gets its backup back, and the restored content is checked against the original's hash. |
| **Manage Replace Backups…** | Lists the jobs in the recovery folder with their age and how many files were replaced, restored, failed or in conflict. |
| **Delete Selected Replace Backup** | Deletes the selected job's receipt and backups after confirmation. |
| **Delete Old Replace Backups** | Applies the retention policy now. |

A rollback never overwrites a file that changed after the job: such a file is marked as a conflict and keeps its current content. A temporary failure, for example a file that is open or locked, is marked as failed and can be retried. A file without a backup is reported as skipped. Close a file in Bareline before rolling it back.

Retention: Bareline keeps the newest 20 finished jobs and removes finished jobs older than 30 days. Interrupted jobs and jobs that still wait for a rollback are always kept.

## After a crash

At the next start Bareline checks the jobs in the recovery folder. A job that stopped before finishing is reconciled by comparing each file's hash with the receipt, and is reported as "Interrupted replace" with the option to roll it back.

## Limits

| Limit | Value |
| --- | --- |
| Files in one job | 10,000. |
| Large files | Files larger than 16 MiB are previewed and replaced through large-file mode. A regular expression with anchors or look-behind cannot search files larger than 64 MiB ([Search limits](search-and-regex.md#limits)). |
| Network folders | Saving to network locations is not supported yet ([Known issues](../../README.md#known-issues-in-this-preview)). |
