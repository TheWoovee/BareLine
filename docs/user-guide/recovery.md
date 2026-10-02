# Recovery

Bareline keeps recovery journals of unsaved work so that a crash, a forced shutdown or a killed process does not lose what you typed. Recovery is a preview feature: its end-to-end acceptance on clean machines is still pending ([STATUS.md](../STATUS.md)), so keep independent backups of important files.

## What is protected

- **Unsaved edits in open documents**, in the normal editor and in large-file mode, including Untitled documents that were never saved. Bareline writes them to a recovery journal in the background as you edit. Each journal starts from a copy of the document's original text, so it can be restored even if the file on disk later changes or disappears.
- **The session**: open tabs, their order, pins and caret positions are kept in `session.json` and restored at the next start (unless **Restore previous session** is off or you started with `--no-session`).
- **Signing out or shutting down Windows** with unsaved changes: Bareline tells Windows why it is delaying the shutdown, writes the session and waits until each document's latest text has a restorable checkpoint, for at most 3 seconds per Windows request. It never cancels the shutdown.
- **An internal error in Bareline**: before exiting, Bareline waits up to 3 seconds for queued checkpoints to finish and records the outcome in `bareline.crash.log`.
- **Separate windows**: journals of a window started with `--new-instance`, `--no-session` or `--no-extensions` go to the same recovery folder and are offered at the next start.

What is not protected: changes you discard yourself. When you close a document or exit and choose not to save, its recovery data is removed. Recovery is also not a backup of saved files; a saved file is protected only by your own backups.

## Where journals live

Journals are in the `recovery` folder of your profile:

| Launch | Recovery folder |
| --- | --- |
| Installed, or an ordinary source build | `%LOCALAPPDATA%\Bareline\recovery` |
| Portable | `<executable folder>\data\recovery` |
| Portable on a read-only folder | `%LOCALAPPDATA%\Bareline\portable-recovery\<key>` |

The same folder holds the receipts and backups of [Replace in Files](replace-in-files.md) jobs (`replace-*` folders). The **Open Recovery Folder** command in the Command Palette opens it in File Explorer.

Journals contain your document text and file paths. Do not attach them to public bug reports; [SUPPORT.md](../../SUPPORT.md#if-you-might-have-lost-work) explains how to report lost work without sharing them.

## Restoring work

When Bareline starts and finds journals that no running window owns, a notification says how many documents can be recovered. Open the **Recovery Center** from that notification or from the Command Palette (**Ctrl+Shift+P**, then type "Recovery"). It shows one row per document, however many checkpoints that document left behind, with its name, state, time and size.

For the selected row:

| Action | Effect |
| --- | --- |
| **Restore recovered copy** | Opens the recovered text in a tab. Save it to a file to keep it. |
| **Compare with current disk** | Compares the recovered text with the file on disk, when the original file is known. |
| **Export saved edits and gap report** | Writes out what can be recovered when part of the journal is missing, with a report of the gaps. |
| **Delete this recovery** | Asks for a second, explicit confirmation (**Confirm irreversible discard**) and then deletes that document's journals. |
| **Delete all recoveries older than 7 days** | Deletes old journals after confirmation. |

Other recovery commands appear in the Command Palette, and under **File > Document > Recovery**, when they apply: **Restore Latest Recovery**, **Retry Recovery** (when writing a journal failed), and **Save Recovered Document As**.

Save a restored document to a new file first, and compare it with the original before you overwrite anything.

## Journals that cannot be read

Bareline never deletes a journal it cannot read. Such a journal is listed as **Unreadable recovery**, and only deletion is offered for it. If a journal you expected is missing or a restore fails, follow [If you might have lost work](../../SUPPORT.md#if-you-might-have-lost-work) before starting Bareline again, and open a **Data loss or crash** issue.

## Reload and Interpret As

**Reload External Changes** and **Encoding > Interpret As** on an edited document ask for confirmation, then treat the document like a close and a reopen: its old journal and undo history are discarded, and the reloaded document gets a new journal.
