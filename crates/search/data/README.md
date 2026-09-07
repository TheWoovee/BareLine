# Unicode case folding

Pinned Unicode 17.0.0 data from https://www.unicode.org/Public/17.0.0/ucd/CaseFolding.txt.
SHA-256: `ff8d8fefbf123574205085d6714c36149eb946d717a0c585c27f0f4ef58c4183`.
License is preserved in LICENSE-UNICODE. The build script selects C and F mappings for default, non-Turkic full case folding. No network request occurs at build or runtime.

Search preserves original text byte ranges through expansion. A match must cover complete source scalars: `ss` may match `ß`, while a single `s` does not replace half of it. Folding does not imply Unicode normalization or locale-specific casing. The scalar mapping ring is bounded by the capped folded pattern length.

Whole-word categories come from https://www.unicode.org/Public/17.0.0/ucd/UnicodeData.txt (SHA-256: 2e1efc1dcb59c575eedf5ccae60f95229f706ee6d031835247d843c11d96470c). Word characters are categories L, N, M, Pc plus U+200C/U+200D. This is an explicit editor word-character policy, not locale-specific segmentation or a claim of complete UAX #29 word breaking.
