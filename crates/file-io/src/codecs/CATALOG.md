# Codec catalog v2

Core Unicode and Latin-1 adapters are Bareline v0.1.0. Legacy mappings use pinned encoding_rs 0.8.35 (WHATWG indexes), except OEM 437/850/852, which are Bareline tables (`oem.rs`). Catalog v2 (BIZ-09) added every non-stateful single-byte encoding encoding_rs ships plus OEM 437/850/852; `Encoding::ALL` lists the catalog. Stateful encodings are unsupported. Labels are ASCII case-insensitive; underscores normalize to hyphens.

| Encoding | Accepted labels | BOM |
| --- | --- | --- |
| UTF-8 | utf-8, utf8 | EF BB BF |
| UTF-16 LE/BE | utf-16le, utf16le / utf-16be, utf16be | FF FE / FE FF |
| UTF-32 LE/BE | utf-32le, utf32le / utf-32be, utf32be | FF FE 00 00 / 00 00 FE FF |
| ISO-8859-1 | iso-8859-1, latin1, latin-1 | none; true U+0000–U+00FF mapping |
| Windows-1250–1258 | windows-125N, cp125N for N=0–8 | none |
| Shift-JIS | shift-jis, sjis, windows-31j | none |
| GBK | gbk, cp936 | none |
| Big5 | big5, big-5 | none |
| EUC-JP | euc-jp | none |
| EUC-KR | euc-kr, windows-949 | none |
| ISO-8859-2, -3, -4, -5, -6, -7, -8, -10, -13, -14, -15, -16 | iso-8859-N, iso8859-N; latin2, latin3, latin4, latin6 (-10), latin7 (-13), latin8 (-14), latin9 (-15), latin10 (-16) | none |
| KOI8-R / KOI8-U | koi8-r, koi8r / koi8-u, koi8u | none |
| Windows-874 (Thai) | windows-874, cp874 | none |
| Mac Roman / Mac Cyrillic | macintosh, x-mac-roman, mac-roman / x-mac-cyrillic, mac-cyrillic | none |
| OEM 437 / 850 / 852 / 866 | cp437, ibm437 / cp850, ibm850 / cp852, ibm852 / cp866, ibm866 | none; OEM 866 is encoding_rs IBM866 |

ISO-8859-9 and ISO-8859-11 have no entry: encoding_rs (WHATWG) decodes them as Windows-1254 and Windows-874, which the catalog offers. ISO-8859-8-I shares ISO-8859-8's bytes.

Single-byte tables decode one byte at a time; bytes 0x00–0x7F are ASCII. A byte a table leaves unassigned (ISO-8859-3/-6/-7/-8, Windows-874, -1253, -1255 and -1257 have some) decodes to U+FFFD with opaque provenance that keeps the exact byte, like any malformed legacy sequence; OEM 437/850/852/866, ISO-8859-1 and the other tables assign all 256 bytes. Every assigned byte maps to a distinct scalar, so encoding is its exact inverse; scalars outside the table are refused as unrepresentable, never replaced or best-fit. Catalog v2 entries are never auto-detected: detection still chooses only Unicode, the scored CJK candidates or the Windows-1252 fallback. The application's Encoding menu lists Unicode first, then one submenu per family (Western European, Central European, Cyrillic, Greek, Turkish, Baltic, Arabic, Hebrew, Vietnamese, Thai, East Asian, DOS/OEM, Mac) under both Interpret As and Convert To.

Decoder strips only the selected encoding's initial BOM, reporting its original range as an empty text span. Encoder emits a requested BOM once. Callers implement Preserve by passing the original BOM state; Emit/Omit map to true/false. Internal U+FEFF remains ordinary text.

Detection inspects at most 64 KiB: longest BOM first (FF FE 00 00 is UTF-32LE only when the sample is whole valid 32-bit scalars; otherwise it is a UTF-16LE BOM before U+0000), then BOM-less UTF-16 LE/BE from a one-sided NUL pattern (at least 20% of units NUL on one side, at most 2% on the other, paired surrogates, few controls; Utf16Sample), then a strict UTF-8 sample (incomplete last scalar allowed), then scored legacy trials. Every candidate among GBK (GB18030 decoder), Big5, EUC-JP, Shift-JIS and EUC-KR that decodes the whole sample (a unit cut by the 64 KiB limit is allowed) is scored by its share of common characters for its script. The best candidate is advisory LegacySample evidence when it has at least 12 non-ASCII scalars, a 15% share and twice the runner-up's share. Otherwise the sample uses Windows-1252 with LegacyFallback confidence and records up to three candidates with at least a 10% share; the application shows them as an "encoding may be wrong" hint until the user interprets the bytes. Neither legacy confidence claims certainty. Later invalid bytes retain opaque provenance instead of changing encoding.

Decoder retains at most four input bytes across pushes and publishes exact raw ranges, including malformed sequences. Whole valid units decode in bulk (at most 16 KiB of raw input per span outside UTF-8); only the BOM, invalid units and units split across pushes or a full sink are read one unit at a time, with the same unit rules, so the joined text and the opaque spans do not depend on chunking. GBK reads GB18030 four-byte units as the GBK decoder accepts them. Provenance maps record runs, not scalars: a run of constant unit widths, an invalid unit, or a mixed-width run of valid units of at most 16 KiB raw whose inner unit boundaries a save recovers by walking its units (provenance map v2, `BLMAP002`; v1 maps stay readable). Genuine replacement/private-use scalars never acquire opaque provenance. Encoder refuses opaque spans and unrepresentable scalars. Streaming encoder retries require resubmitting unconsumed spans unchanged; consumed counts whole spans, while its internal cursor remembers partially written spans. Sink writes must be all-or-error; output errors terminate that stream/staged save.

Byte-identical untouched saves copy original source ranges, including valid legacy aliases/noncanonical mappings; re-encoding alone cannot guarantee bijection. ResidentEncoding retains the immutable baseline snapshot and raw bytes; piece identity locates original ranges through splits, edits and undo. Its private original encoding governs raw-copy compatibility independently of public mutable save policy. Do not use opaque bytes to bypass conversion refusal. Resident and disk-backed paged paths integrate streaming open/save and conversion through the file-io lifecycle; final native exact-byte qualification remains pending.
