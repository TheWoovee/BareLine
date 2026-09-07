# Bareline Editor

Native Windows text editor, implemented locally from the Bareline v1.3 blueprint.

**Current state:** local development preview. Native hardware/software rendering, multiline UTF-8 editing, undo/redo, selection, clipboard commands and background file operations are wired. Open currently supports UTF-8 files up to 1 MiB; save stages and verifies captured content before replacement. Large-file streaming, recovery and several planned features remain unfinished. Visual fidelity and physical input checks await an unlocked desktop.

```powershell
cargo run -p bareline --locked
cargo run -p bareline --locked -- --software
cargo run -p bareline --locked -- "C:\path\to\note.txt"
```

For an already built binary, launch `target/debug/bareline.exe` directly. Do not rebuild just to relaunch.

Normal launch creates an editable Untitled document after the first frame. Up to 16 file paths can be passed for background opening; use `--` before paths that resemble options. Diagnostic launch modes accept no document paths.

The preserved specification, PR briefs and reference images are in [docs/blueprint](docs/blueprint/README.md). Live progress is in [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md). Start a phase by reading its own PR brief and implementation note, then only the linked contracts needed by that work.

Use focused tests for consequential behavior. `cargo xtask perf smoke` uses the existing debug binary for hidden-window checks; `cargo xtask perf launch` measures visible launch and idle behavior in both renderer modes. Debug samples are not controlled release benchmarks. Deferred desktop verification and its resume plan are tracked in [docs/UNLOCK_CHECKLIST.md](docs/UNLOCK_CHECKLIST.md).

Everything stays local until the owner authorizes a remote push. No remote is configured.

For client-area layout checks without opening a window, run cargo xtask render 1 (or 1.5 / 2). Reuse target/debug/xtask.exe render after the first compile. Generated BMP fixtures are saved under tests/visual/results and are not native-window screenshots.
