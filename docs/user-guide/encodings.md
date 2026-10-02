# Encodings

Bareline keeps the original bytes of every file it opens and shows them as text through an encoding. The **Encoding** menu has two different operations on that encoding, and it matters which one you use.

## Interpret As versus Convert To

| | **Encoding > Interpret As** | **Encoding > Convert To** |
| --- | --- | --- |
| Use it when | The text looks wrong because Bareline guessed the wrong encoding. | The text looks right and you want the file saved in another encoding. |
| What it does | Decodes the file's **original bytes** again with the encoding you choose. | Keeps the text and changes the encoding used for the **next save**. |
| The file on disk | Unchanged. Nothing is written until you save. | Rewritten in the new encoding when you save. |
| Your edits | Interpret As reloads the document: on an edited document it asks first, then discards the edits, the undo history and the recovery journal, like closing and reopening ([Recovery](recovery.md#reload-and-interpret-as)). | Kept. |
| Characters that do not fit | Bytes that are invalid in the chosen encoding are shown as replacement characters (U+FFFD) but keep their original bytes, which an unchanged save writes back exactly. | A save refuses characters the target encoding cannot represent, and invalid original bytes; nothing is replaced or approximated. **Show Encoding Save Failure** explains a refused save. |

Both submenus list Unicode encodings first, then one submenu per region or script: Western European, Central European, Cyrillic, Greek, Turkish, Baltic, Arabic, Hebrew, Vietnamese, Thai, East Asian, DOS/OEM and Mac. **Character Sets…** shows the same list in a picker.

When a file is too large for the normal editor in the encoding you choose with Interpret As, Bareline reopens it in [large-file mode](large-files.md) with that encoding. Converting a large file to another encoding uses temporary disk space, up to the **Temporary disk space for encoding conversion** setting (20 GiB by default).

## Supported encodings

UTF-8, UTF-16 LE/BE, UTF-32 LE/BE, ISO-8859-1 to ISO-8859-16 (except -9, -11 and -12), Windows-874 and Windows-1250 to Windows-1258, KOI8-R, KOI8-U, Mac Roman, Mac Cyrillic, OEM 437, 850, 852 and 866, Shift-JIS, GBK, Big5, EUC-JP and EUC-KR. Stateful encodings such as ISO-2022-JP and UTF-7 are not supported. The [codec catalog](../../crates/file-io/src/codecs/CATALOG.md) lists the aliases and how each codec preserves bytes.

## Detection

When a file opens, Bareline reads up to its first 64 KiB:

1. A byte order mark selects UTF-8, UTF-16 or UTF-32.
2. UTF-16 without a byte order mark is recognized by its pattern of zero bytes; then valid UTF-8 opens as UTF-8.
3. Otherwise GBK, Big5, Shift-JIS, EUC-JP and EUC-KR are scored by how many common characters of their script the sample decodes to. One of them wins only when the sample has enough non-ASCII characters and the winner is clearly ahead of the others.
4. If no candidate wins, the file opens as Windows-1252 and the status bar shows an "encoding may be wrong" hint naming up to three likely encodings. Use **Interpret As** to pick one.

ISO-8859, KOI8, Mac and OEM encodings are never detected automatically; choose them with **Interpret As**. A file that looks binary opens read-only with a notice offering **Edit as text** and **Close**.

## Byte order marks and line endings

- **Write Byte Order Mark** and **Omit Byte Order Mark** set whether the next save writes a BOM.
- **Line Endings…** converts the document, or the selection, to CRLF, LF or CR. Conversions of any size are one undoable step. In a document with mixed line endings, new lines use the most frequent ending.
- Settings **Encoding for new files** and **Line endings for new files** set the defaults for new documents.

Regular expressions treat CRLF, LF and CR each as one line break ([Search, replace and regular expressions](search-and-regex.md#regular-expression-semantics)).
