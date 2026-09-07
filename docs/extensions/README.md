# First-party component walkthrough

The SDK and extensions use MIT OR Apache-2.0. The editor and optional host use MPL-2.0.
The optional Wasmtime 48 host requires Rust 1.95. The editor retains its Rust 1.85
contract. A Windows machine with the MSVC tools and the repository toolchain is
required for the authenticated child integration harness.

## Build and execute the reference components

Run `./scripts/test-first-party.ps1` from PowerShell. It installs the
`wasm32-wasip2` target, builds all three release components, and explicitly runs
the ignored `first_party` integration test. This test starts the real native host
in a restricted job, authenticates the named pipe, and exercises broker reads,
panel replies and staged edits from the generated components. The test supplies
local fixture trust below production publisher verification; it does not enable
unsigned runtime loading in the editor.

`./scripts/package-first-party.ps1` creates flat `.blex` archives under
`target/extension-packages`, each containing `manifest.toml` and `extension.wasm`,
and prints their SHA256 hashes. These are unsigned development artifacts. Their
creation does not publish a catalog or grant installation authority.

## Copy the JSON formatter

Start with `extensions/json-tools` and its Cargo.toml, changing the package name
and stable manifest ID. Keep `crate-type = ["cdylib", "rlib"]`, the SDK dependency,
and the `wasm32` Guest implementation/export at the end of lib.rs. The shared
reference client in `extensions/common` receives a versioned Invocation, streams
snapshot bytes, and sends postcard broker envelopes through the WIT interface at
`crates/extension-sdk/wit/extension.wit`.

The manifest declares schema 1, protocol minimum/maximum 1, component entry name,
commands, panels, publisher, version and requested capabilities. Capabilities use
the protocol enum names (`DocumentRead`, `DocumentEdit`, `UiPanel`). Manifest
requests never grant permission; the editor must obtain explicit user approval.
JSON/XML request these three capabilities. Hex omits DocumentEdit.

Invocation identifies one document/revision, original-byte generation and current
grant generation. ReadTextRange returns text-view bytes; 64 KiB chunks may split a
UTF-8 scalar and must be assembled by the stream consumer. ReadOriginalBytes uses
the original disk generation and RawRange. Hex never substitutes unsaved edits.

For formatting, count output first, then create Staged, write the validated output
and finish. BeginEdits fixes the base revision; AppendChunk has a 1 MiB protocol
ceiling; CommitEdits proposes one atomic transaction. The reference client caps
formatter output at 60 MiB before starting. The parent keeps proposed edits until
the child exits successfully, then rechecks document identity/revision through
the actor. Failure, cancellation, revocation or stale revision discards proposals.
Documents retaining undecodable original bytes require a preservation/loss policy
before replace-all formatting can be allowed.

Panel output must use an ID declared in the manifest and remain within the bounded
output quota. Network, processes, directories and inherited stdio are unavailable
in the default host. XML disables DTD/entities/XInclude and has a deliberately
limited namespace-aware XPath subset; mixed content and xml:space are preserved.

## Signing and release

Production sideload requires signed catalog metadata verified with the configured
owner minisign public key, matching package size/hash/protocol and safe extraction.
The native runtime additionally requires pinned Authenticode publisher validation.
A clean author walkthrough, signed first-party catalog publication and production
runtime distribution remain release acceptance tasks; no production keys or online
activation are supplied by these scripts. Submit catalog additions through the
owner-reviewed catalog repository only after release authorization.
