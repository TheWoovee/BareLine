# Contributing to Bareline

Bug reports, focused fixes, tests, and documentation improvements are welcome. Bareline is currently a Windows x64 preview. Before a substantial feature or dependency change, open an [issue](https://github.com/TheWoovee/BareLine/issues) describing the problem and proposed approach so maintainers can discuss scope.

Follow the [community conduct policy](CODE_OF_CONDUCT.md). For security-sensitive reports, follow [SECURITY.md](SECURITY.md) and avoid posting private documents, credentials, or sensitive paths in a public issue.

## Set up a development build

Use Windows x64, rustup, and Visual Studio C++ Build Tools with the Desktop development with C++ workload and Windows SDK. The repository pins Rust **1.98.1**, rustfmt, and Clippy through [rust-toolchain.toml](rust-toolchain.toml). The native syntax bridge requires C++17. See the [README build instructions](README.md#build-from-source) for cloning or building a source ZIP.

```powershell
cargo build --locked -p bareline
cargo run --locked -p bareline
```

To exercise software rendering:

```powershell
cargo run --locked -p bareline -- --software
```

After building, launch `target\debug\bareline.exe` directly to reuse the binary. Test against generated or disposable files. For an isolated portable profile, copy the executable into a writable scratch directory, create an empty `bareline.portable` file beside it, and keep its generated `data` folder separate from your normal profile.

The normal build is a preview with update downloads and extension execution disabled. Do not enable test fixture trust in a public build. Configured releases and first-party extension qualification have separate tooling under `packaging`, `release`, and `scripts`.

## Make a focused change

- Keep a pull request centered on one problem. Explain the user-visible behavior before and after the change.
- Preserve source license headers and existing style. Do not mix unrelated formatting or generated artifacts into a fix.
- Add behavioral tests for consequential changes, especially document edits, file saves, recovery, encodings, search/replacement, and state transitions. A documentation-only change normally needs a link/content check, not a full build.
- Preserve document contents and original-byte handling across failures. Do not silently turn a partial, cancelled, or unsupported operation into success.
- Keep Windows API types inside the Windows platform and renderer implementation boundaries. Shared models and algorithms should remain portable.
- Keep the editor event-driven. Do not add a browser or asynchronous runtime to the editor process. Blocking file work belongs on the existing worker paths.
- Keep startup work bounded. Document loading and other expensive work should follow the first frame rather than delay it.
- Explain why a new dependency is needed, its license, and its effect on the build and runtime. Keep `Cargo.lock` consistent with deliberate dependency changes.

For UI changes, check the relevant screen in light and dark themes, appropriate display scaling, keyboard navigation, and both rendering modes when the change affects rendering. Report what you actually exercised; an automated model test does not establish native input, accessibility, or visual correctness.

## Validate the affected code

Start with the affected package and reuse incremental builds. For example, a search change can begin with:

```powershell
cargo test --locked -p bareline-search
cargo check --locked -p bareline
cargo fmt --all --check
```

Replace `bareline-search` with the package you changed. Run integration or broader tests when shared interfaces, failures, or the scope of the change warrant them:

```powershell
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
```

The [Correctness workflow](.github/workflows/ci.yml) is the reference for CI checks. It includes a Clippy baseline check, so existing warning debt is not a reason to add new warnings or replace the baseline wholesale. Non-Windows jobs check shared code; they do not qualify a native Linux or macOS desktop release.

Python 3.12 is used by CI's validation scripts. After editing Rust version declarations, manifests, or workflow toolchains, run:

```powershell
python .github/workflows/check_toolchain_contract.py
```

For platform-boundary changes, also run:

```powershell
python .github/workflows/check_portability.py
```

Include commands, results, and any untested behavior in the pull request. Keep performance measurements tied to the build mode and environment; do not present debug samples as release benchmarks.

## Submit a pull request

1. Fork the repository and create a branch for the change.
2. Make the smallest complete fix and run the relevant checks.
3. Sign off commits with `git commit -s` to certify the [Developer Certificate of Origin](https://developercertificate.org/).
4. Open a pull request against the repository's default branch with the problem, implementation, validation, and known limitations. Link the related issue if one exists.

For a bug report, include the application version, Windows version/build, hardware or software renderer, exact reproduction steps, expected and actual behavior, and a minimal non-sensitive sample if needed. For crashes or data loss, include the sequence of edits and file operations without publishing personal recovery journals.

Core contributions use [MPL-2.0](LICENSE). SDK/protocol and applicable extension contributions use MIT OR Apache-2.0 according to their existing SPDX headers and [LICENSE-SDK](LICENSE-SDK). Preserve third-party notices and attribution when changing bundled code.
