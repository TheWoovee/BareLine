# Bareline Editor

Native Windows text editor, implemented locally from the Bareline v1.3 blueprint.

**Current state:** local development preview for Windows 10 22H2 build 19045 and
Windows 11 23H2 build 22631 or later on x64. Those compatibility floors still
need final clean-machine qualification. Hardware and software rendering,
resident and paged editing, background file operations, recovery, sessions,
search, syntax, workspace views and the command surfaces are integrated. The
[current capability ledger](docs/parity/index.json) records the exact state,
owner, code, tests, limitation, next step and pending evidence identity for all
52 parity families. It deliberately does not turn focused or historical results
into final acceptance.

Development and release builds use Rust 1.98.1, pinned by rust-toolchain.toml.
That compiler is the workspace's currently supported Rust version. Older compilers
are not a supported minimum until a separate locked compatibility run qualifies one.

```powershell
cargo run -p bareline --locked
cargo run -p bareline --locked -- --software
cargo run -p bareline --locked -- "C:\path\to\note.txt"
```

For an already built binary, launch `target/debug/bareline.exe` directly. Do not rebuild just to relaunch.

Normal launch creates an editable Untitled document after the first frame. Up to 16 file paths can be passed for background opening; use `--` before paths that resemble options. Diagnostic launch modes accept no document paths.

The default large-file threshold is 256 MiB. Files at or below it use the
resident editor; larger files use the paged path with 1 MiB pages, a 64 MiB
per-document page cache and a 256 MiB aggregate document cache. Undo uses a
separate 128 MiB aggregate RAM budget and keeps at most 100,000 changes per
document. Encoding conversion may use up to 20 GiB of temporary storage. These
are defaults, not file-size claims: the corresponding settings accept a
resident threshold up to 1 TiB, pages up to 16 MiB, a per-file cache up to
1 GiB, and aggregate/undo/transcode budgets up to 1 TiB. The file source uses a
64-bit length and has no separate 1 MiB open limit; success still depends on
addressable storage, configured budgets, free disk space and the underlying
filesystem.

Supported text encodings are UTF-8, UTF-16 LE/BE, UTF-32 LE/BE, ISO-8859-1,
Windows-1250 through Windows-1258, Shift-JIS, GBK, Big5, EUC-JP and EUC-KR.
Stateful encodings are unsupported. Detection reads at most 64 KiB and ambiguous
legacy input falls back to Windows-1252. See the
[codec catalog](crates/file-io/src/codecs/CATALOG.md) for aliases, BOM behavior
and opaque-byte rules. Unchanged same-encoding saves preserve original byte
ranges. Changed saves stage captured document content, check destination/source
identity, flush and verify before replacement. Automatic resident and paged
recovery uses bounded, checksummed local journals; explicit discard must become
durable before recovery ownership is retired. The PR-U05 Recovery Center source
correction is integrated with focused receipts; final native recovery and AC
qualification remain pending.

Preview builds fail closed for downloads, updates and first-party extension
execution unless a reviewed public trust configuration is compiled in. They do
not assume fixture trust, and an unconfigured command inventory must not claim
dynamic extension commands. PR-T11's configured release source is integrated;
the first connected run built three release binaries before its tooling corrections,
while the corrected connected fixture, actual installation, installed Fast run,
signing and external acceptance remain pending. The [parity evidence guide](docs/parity/README.md) explains the composed registry
export and checked T09/native receipt import. Final integrated acceptance and
comparative performance qualification remain pending.

The preserved specification, PR briefs and reference images are in [docs/blueprint](docs/blueprint/README.md). Live progress is in [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md). Start a phase by reading its own PR brief and implementation note, then only the linked contracts needed by that work.

Use focused tests for consequential behavior. `cargo xtask perf smoke` uses the existing debug binary for hidden-window checks; `cargo xtask perf launch` measures visible launch and idle behavior in both renderer modes. Debug samples are not controlled release benchmarks. Deferred desktop verification and its resume plan are tracked in [docs/UNLOCK_CHECKLIST.md](docs/UNLOCK_CHECKLIST.md).

Everything stays local until the owner authorizes a remote push. No remote is configured.

For client-area layout checks without opening a window, run cargo xtask render 1 (or 1.5 / 2). Reuse target/debug/xtask.exe render after the first compile. Generated BMP fixtures are saved under tests/visual/results and are not native-window screenshots.
