# Search, replace and regular expressions

## Finding text

| Task | How |
| --- | --- |
| Find in the current document | **Ctrl+F** opens the find bar. **F3** and **Shift+F3** go to the next and previous match. |
| Replace in the current document | **Ctrl+H**. **Replace Selected Match** replaces the current match; **Replace All in Current Document** replaces every match as one undoable step. |
| Search only the selection | **Search > Scope > Find in Selection**. |
| Find in all open documents | **Ctrl+Shift+F** (**Find in Open Documents**). Results are listed in the search results panel. |
| Find in a folder | **Search > Scope > Find in Folder…** asks for a folder, then opens a panel that takes the search text (using the current case, word and regex options), file extensions to include, and directory names to exclude (`.git, .svn, .hg` by default). Binary files are skipped unless you include them. |
| Replace in a folder | [Replace in Files](replace-in-files.md). |
| Highlight matches | **Search > Mark** has five mark styles and commands to clear each one. |

A search that stops early says so: the find bar shows "Incomplete: …" and the results panel "Results incomplete · …" with the reason (a time, memory, backtracking or nesting-depth limit of the regular-expression engine, the result limit, or an unavailable source). Never treat such a result as the complete list of matches.

## Modes and options

| Mode | Meaning |
| --- | --- |
| **Literal** | The text as typed. |
| **Extended** | The text with the escapes `\n`, `\r`, `\t`, `\0`, `\\`, `\xHH` and `\uHHHH`. A malformed escape is refused. |
| **Regex** | A PCRE2 regular expression in Unicode mode. |

Options in the find bar and under **Search > Options**: **Match Case**, **Whole Word**, and, in Regex mode, **. Matches Newline**. **. Matches Newline** is recorded in macros and applies to current-document, open-document and folder searches.

## Regular-expression semantics

Bareline matches regular expressions line by line, like Notepad++ (owner decision D1, changed in this preview):

- `^` and `$` match at the start and end of every line, not only of the document. Start the pattern with `(?-m)` to anchor at the document start and end instead, or use `\A` and `\z`.
- CRLF, LF and a lone CR each end a line. `$` matches before the CR of a CRLF and never between the CR and the LF, so `\s+$` removes trailing spaces without touching the line ending.
- `.` does not match CR or LF unless **. matches newline** is on. `\R` matches CRLF, LF or CR.
- An empty match steps over a CRLF as one unit, so Replace All never splits a CRLF and keeps the document's line endings.
- **Whole Word** is checked at the end of every alternative: with Whole Word on, `x|xy` finds `xy`.
- **Match Case** off folds one character to one character. `strasse` finds `STRASSE` but not `Straße`. Literal and Extended searches use full case folding, so there `strasse` also finds `Straße`.

### Replacement text

In Regex mode the replacement text understands:

| Syntax | Inserts |
| --- | --- |
| `$0` | The whole match. |
| `$1`, `${1}`, `\1` | A numbered group. |
| `${name}`, `$+{name}` | A named group. |
| `$$` | A literal `$`. |
| `\n`, `\r`, `\t`, `\\` | Line feed, carriage return, tab, backslash. |

Any other escape is refused rather than inserted literally. Case-changing escapes such as `\U` and `\L` are not supported. In Extended mode the replacement uses the Extended escapes above; in Literal mode it is inserted as typed. Preserving the case of each match is available in [Replace in Files](replace-in-files.md).

## Limits

| Limit | Value |
| --- | --- |
| Pattern length | 64 KiB. |
| Text pasted into the find field | 16 KiB. |
| Time for one match attempt | 2 seconds plus time for the text it may scan. Many quick matches never share one time budget. |
| Engine limits | Backtracking, nesting depth and memory are bounded; the result names the limit that stopped a search. |
| Context for anchored patterns | A pattern that uses `^`, `$`, `\b`, `\B`, `\A`, `\G`, `\X`, look-behind or backtracking verbs is matched against the whole text, which is limited to 64 MiB. Larger documents and files report incomplete results for such patterns. Other patterns are searched window by window. |
| Replace All in large-file mode | At most 10,000 replacements; more are refused before anything changes ([Large files](large-files.md)). |
| Replace All in the normal editor | One undoable step. If the replacements need more than 64 MiB of staged text, the command is refused with a message asking you to narrow the scope. |
