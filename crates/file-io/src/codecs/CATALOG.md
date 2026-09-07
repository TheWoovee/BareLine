# Codec catalog v1

Core Unicode and Latin-1 adapters are Bareline v0.1.0. Legacy mappings use pinned encoding_rs 0.8.35. Stateful encodings are unsupported. Labels are ASCII case-insensitive; underscores normalize to hyphens.

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

Decoder strips only the selected encoding's initial BOM, reporting its original range as an empty text span. Encoder emits a requested BOM once. Callers implement Preserve by passing the original BOM state; Emit/Omit map to true/false. Internal U+FEFF remains ordinary text.

Detection inspects at most 64 KiB: longest BOM first, strict UTF-8 sample (incomplete last scalar allowed), then bounded legacy trials. Two Japanese kana or Korean Hangul scalars in a strict trial provide advisory LegacySample evidence. Ambiguous samples use Windows-1252 with LegacyFallback confidence; neither legacy confidence claims certainty. Later invalid bytes retain opaque provenance instead of changing encoding.

Decoder retains at most four input bytes across pushes and publishes exact raw ranges, including malformed sequences. Genuine replacement/private-use scalars never acquire opaque provenance. Encoder refuses opaque spans and unrepresentable scalars. Streaming encoder retries require resubmitting unconsumed spans unchanged; consumed counts whole spans, while its internal cursor remembers partially written spans. Sink writes must be all-or-error; output errors terminate that stream/staged save.

Byte-identical untouched saves copy original source ranges, including valid legacy aliases/noncanonical mappings; re-encoding alone cannot guarantee bijection. ResidentEncoding retains the immutable baseline snapshot and raw bytes; piece identity locates original ranges through splits, edits and undo. Its private original encoding governs raw-copy compatibility independently of public mutable save policy. Do not use opaque bytes to bypass conversion refusal. Generic resident OpenStreaming/SaveEncoded integrate this behavior; disk-backed paged transcode remains a later slice.
