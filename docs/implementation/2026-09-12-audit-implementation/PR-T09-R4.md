# PR-T09 R4 — Source identity framing parity

- Base: `f556f5d10acfe8757d7e71bb46ed19858e8a981c`
- Scope: make the xtask journey source manifest byte-for-byte compatible with the T09 Python evidence producer while preserving result/capture binding.
- Reproduction: the Rust producer frames `untracked\0` once for the collection, while both Python producers frame it before each untracked path. Multiple untracked files therefore produce different hashes for one frozen checkout.
- Result binding: xtask writes the final result before emitting its absolute path and exact file SHA-256. The T09 wrapper retains those lines, and the adapter requires both values to match the supplied result, so no publication change is needed.
- Verification: a cross-language fixture will compare the Rust manifest helper with the actual Python T09 helper for no untracked files and multiple untracked files including a non-ASCII path and multi-chunk content. Root owns Cargo checks and native journey reruns.
