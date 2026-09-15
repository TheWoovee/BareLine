# Windows installer and extractable package — 2026-09-08

Scope: assemble local unsigned Windows installer and portable ZIP under PR-018 / ADR-31. Preserve the existing per-user default with optional machine scope and the empty `bareline.portable` marker that selects adjacent data storage.

Packaging audit: both package types include the editor, update helper, LICENSE, dependency notices and SBOM. The portable ZIP alone includes the marker. Harden package preflight so a missing/wrong compiler or existing installer is rejected before creating the ZIP, and existing signed inventories cannot be replaced accidentally. Correct the payload documentation to include SBOM.

Evidence: `pwsh -NoProfile -File packaging/windows/test-local.ps1` passed on 2026-09-08. It checks deterministic ZIP bytes, six entries and empty marker, unchanged extracted payload bytes, refusal of overwrite/missing payload, compiler preflight before ZIP creation, retained signed output refusal, and complete deterministic final inventory. Actual executable compilation, package production and native acceptance are coordinated by the parent task; this note does not claim release signing or publication.

Actual compiler probe: genuine Inno Setup 6.4.3 exposes zero PE product/file version fields. The pin now uses ISPP `Ver == EncodeVer(6, 4, 3)` in a `/O-` probe and again in the installer source. The isolated official 6.4.3 compiler passed (exit 0); the separately installed newer compiler was rejected (exit 2). No installer was emitted by either probe.

Notices can now select validated packaged roots. The default still includes editor, helper and optional extension host; editor/helper-only packages use `-Roots bareline,bareline-update-helper` and record that scope in generated notices. Actual notices generation is performed by the parent task.

Actual cargo-cyclonedx 0.5.9 inputs required recursive component normalization: local `download_url=file://` qualifiers are removed from package URLs while target source fragments and Cargo target discriminators remain. Every nested component reference is remapped before dependency normalization. The developer-path guard now distinguishes Windows drive paths from ordinary HTTPS URLs (the previous expression incorrectly matched the trailing `s:/` in `https://`). Generation passed for editor/helper inputs into `target/package-20260908/payload/SBOM.json`: 209 unique component references, 126 dependency rows with all references resolved, and 128 top-level package/native components with license evidence. No packaging files changed after that SBOM generation.
