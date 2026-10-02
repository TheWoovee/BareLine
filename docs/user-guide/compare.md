# Compare

Compare is built into Bareline; no plugin is needed. It shows two texts side by side with added, removed, changed and moved lines marked, and lets you copy changes from one side to the other. Compare is a preview feature.

## Starting a comparison

All commands are under **Tools > Compare** and in the Command Palette.

| Command | Compares |
| --- | --- |
| **Compare documents** | Two open documents. **Change left source** and **Change right source** pick other documents; **Swap compare sources** swaps the sides. |
| **Compare with disk file…** | The active document with a file you choose. |
| **Compare with last saved version** | The active document with the text it had when it was last saved. |
| **Compare with current disk version** | The active document with its file as it is on disk now, for example after another program changed it. |

The Recovery Center can also compare a recovered document with its file on disk ([Recovery](recovery.md)).

## Working with differences

- **Next difference** and **Previous difference** move between changes. The status shows the number of differences, or "No differences".
- **Copy left to right** and **Copy right to left** copy the current difference to the other side. **Copy selected range left to right** and **right to left** copy only the selected lines. Each copy is one undoable edit in the target document.
- Edits made while comparing mark the result "Sources changed · Recompare". **Recompare** runs it again; **Pause automatic recompare** stops automatic runs while you edit.
- **Synchronize horizontal scrolling** keeps both sides at the same horizontal position.
- **Close compare** ends the comparison.

## Options

**Compare Options** sets what counts as a difference:

| Option | Effect |
| --- | --- |
| **Compare whitespace mode**, **Ignore leading/trailing whitespace**, **Ignore all whitespace** | How spaces and tabs are compared. |
| **Ignore blank lines** | Blank lines added or removed are not differences. |
| **Ignore case** | Letter case is not a difference. |
| **Ignore EOL style** | CRLF, LF and CR endings compare equal. |
| **Ignore encoding BOM** | A byte order mark is not a difference. |
| **Normalize tabs** | Tabs compare as spaces. |

**Compare Colors** sets the background, accent and gutter colors of each kind of difference, offers a **Color-blind compare palette**, and has light, dark and system themes.

## How large inputs are compared

| Input | Behavior |
| --- | --- |
| Byte-identical documents in the normal editor | "No differences", whatever their size. |
| Normal editor, up to 200,000 lines and 64 MiB of combined text | Exact. Lines that occur once on each side anchor the alignment, and the text between anchors is compared line by line, with changes inside a line marked. Each step has 5 seconds. |
| A region between anchors that exceeds a step's time or memory limit | Shown as one changed block between its anchors; the rest of the result stays exact. The status reads "Coarse comparison". |
| Normal editor, above 200,000 lines or 64 MiB | One changed block for the whole file ("Coarse comparison"). |
| Large-file mode | Both files are hashed line by line and aligned on lines that occur once on each side, then compared window by window. Large insertions and removals are reported where they are. When the files have more distinct lines than the memory limit can index, the alignment uses a sample of them and the status reads "Coarse comparison (memory limit)". |

A coarse block is a correct statement that the region differs, not a line-by-line result. Folder comparison and inline (single-pane) diff are not available; they are planned after 1.0 ([ROADMAP.md](../../ROADMAP.md)).
