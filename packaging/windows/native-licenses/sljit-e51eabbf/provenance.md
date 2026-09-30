# Upstream license provenance

Component: SLJIT (stack-less just-in-time compiler) at commit
`e51eabbfb8eabc6526f56e4e88b29fb10d1ee048`. Declared license: BSD-2-Clause.

PCRE2 10.46 pins SLJIT as the `deps/sljit` submodule at that commit. The locked
`pcre2-sys` 0.2.10 crate vendors it under `upstream/deps/sljit/sljit_src` and compiles it
into the editor because `SUPPORT_JIT` is enabled for x86-64. The vendored `sljitLir.c` and
`sljitNativeX86_64.c` have the same git blobs as that commit. The crate archive omits the
SLJIT `LICENSE`. SLJIT has no release version; the commit identifies it.

Source: [e51eabbf LICENSE](https://github.com/zherczeg/sljit/blob/e51eabbfb8eabc6526f56e4e88b29fb10d1ee048/LICENSE),
git blob `0aaecaaa28677aa1d3d69025ffab957131eca25d`. Retrieved 2026-09-30. No text was
modified.

SHA-256: `5f216505c0f6ea3273caec89e766eef93cdeb7bbb0c429f9360116d7c938feeb`. Bytes: 1444.
