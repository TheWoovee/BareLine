# First-party component walkthrough

The SDK and extensions use MIT OR Apache-2.0. The editor and optional host use MPL-2.0.
Wasmtime 48 and its host dependencies require at least Rust 1.95. The host still
inherits the workspace's supported Rust 1.98.1 compiler; the dependency floor is
not a separately qualified workspace minimum. A Windows machine with the MSVC
tools and the repository toolchain is required for the authenticated child
integration harness.

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

## PR-027 bounded command navigation

The extension manager passes its multiline argument field verbatim as
`Invocation.arguments`. Arguments are limited to 4096 bytes. Running a command
uses the current document revision and permission generation; copying a tree
cursor does not grant access to another document.

`ext.json.tree` now scans a real structural page, with at most 64 KiB of input,
256 node starts, and nesting depth 128 per invocation. It shows container and
scalar start offsets. String tokens are labelled `string/key` because this view
is a structural preview, not a full semantic validation result. Run
`ext.json.validate` to validate the entire JSON grammar. A page supplies an exact
multiline continuation block; paste it into the argument field and run the tree
command again. To explore a displayed container, use `start=<TextOffset>`.
Optional `end` bounds the selected range. Continuation uses `cursor`, `depth`,
`mode`, and `escaped`; copy these together. Unknown, duplicate, and out-of-range
numeric arguments are rejected. The view never materializes the whole source.

For `ext.xml.xpath`, the first line is the expression; subsequent lines bind
prefixes, for example:

```text
/r/a:item[2]/text()
a=urn:example
```

Empty arguments select `/*`. At most 64 unique namespace prefixes are accepted.
Duplicate prefixes and malformed bindings are errors. Descendant positional
predicates are evaluated among matching siblings, and `text()` returns separate
direct text nodes across child elements. DTD, external entities and XInclude
remain disabled. XPath input is limited to 16 MiB, with bounded depth, work,
result count and output size; unsupported syntax returns an error.

For `ext.hex.open` and `ext.hex.goto`, use:

```text
offset=0x1000
rows=32
```

Offsets accept unsigned decimal or `0x` hexadecimal. Rows range from 1 to 256;
the default is 32. A legacy bare offset also works. Hex aligns the viewport to
16-byte rows and reads only that range plus two rows of overscan on each side.
Missing original bytes appear as `??` with the provider's failure message.
Decoded text and unsaved edits are never substituted for unavailable originals.

A failed or abandoned formatter upload now sends a best-effort Cancel for its
staged transaction. The host still owns timeout/revocation cleanup and the final
revision check, including when the pipe is already unavailable.

## Large validation status

The JSON parser streams input and retains only a bounded index. This alone does
not establish that a 1 GiB document completes through the host: its default
interactive 5-second deadline and instruction quota are separate constraints.
An explicitly selected, cancellable background execution budget is being
coordinated with PR-016. Until that path and its actual generated-component test
pass, 1 GiB validation remains an open acceptance item. No command silently
extends its own permissions or execution deadline.

The JSON manifest now declares `background_commands = ["ext.json.validate"]`.
This signed declaration only makes the command eligible for the manager's
explicit **Run Background** action. It does not grant permissions or let guest
arguments select a budget. PR-016 supplies the host-owned bounded background
budget (120 seconds, separate instruction quota) and cancellable Job Object;
ordinary Run keeps the interactive default. This source change is unverified
until the combined component/manager gate, including the generated 1 GiB case.

XML commands consume the editor's decoded UTF-8 text view even when the original
file is UTF-16 or another supported encoding. The original XML declaration is
preserved during formatting; its encoding label does not trigger a second decode.
Encoding-name syntax is validated. The editor remains responsible for encoding
and byte-preservation policy when committing/saving the returned text.
