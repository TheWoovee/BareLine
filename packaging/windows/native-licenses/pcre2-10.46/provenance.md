# Upstream license provenance

Component: PCRE2 10.46 (C library). Declared license: BSD-3-Clause WITH PCRE2-exception.

The locked `pcre2-sys` 0.2.10 crate (Cargo.lock checksum
`18b9073c1a2549bd409bf4a32c94d903bb1a09bf845bc306ae148897fa0760a4`) compiles the PCRE2
sources it vendors under `upstream/` into the editor with `SUPPORT_JIT`. Its `update-pcre2`
script pins PCRE2 10.46 and `upstream/include/pcre2.h` defines `PCRE2_MAJOR 10`,
`PCRE2_MINOR 46`, `PCRE2_DATE 2025-08-27`. The crate archive omits the PCRE2 `LICENCE.md`;
its own COPYING/LICENSE-MIT/UNLICENSE cover only the Rust binding.

Source: [pcre2-10.46 LICENCE.md](https://github.com/PCRE2Project/pcre2/blob/pcre2-10.46/LICENCE.md),
tag commit `b2bd4254b379b9d7dc9a3dda060a7e27009ccdff`, git blob
`f58ceb75a63e5f958631932f83d0786ee7593485`. Retrieved 2026-09-30. No text was modified.
The vendored `src/pcre2_compile.c` has the same git blob at that tag as in the crate.

SHA-256: `9cf7ac6976099a1d856826d3ef1b093bd6b84489dc6100628ac79e740cf9885a`. Bytes: 3867.

Bareline links PCRE2 directly, so the binary-redistribution condition applies to its
packages: THIRD-PARTY-NOTICES.md reproduces this text. The JIT's SLJIT dependency has its
own license in `../sljit-e51eabbf`.
