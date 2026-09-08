# PR-019 Windows adapter handoff

These new modules are source-only and have not been imported, tested or executed
in the current gate. They do not download Notepad++ or select a version implicitly.
The runner owner must integrate and schedule validation before using any output.

- `windows_process_metrics.py`: `OwnedProcessTree(argv, cwd)` starts suspended,
  assigns a kill-on-close Job Object before resuming, and measures only that Job.
  `MemorySampler(tree).start()` / `.stop()` samples live per-process private bytes
  and working sets. `metrics()` rejects incomplete samples. Peaks are maxima of
  sampled aggregate totals, not sums of individual lifetime peaks. Process IDs
  are checked for Job membership after opening and paired with creation times.
  It caps process IDs, sample count and retained identity entries.
- `notepadpp_driver.py`: requires application SHA256, exact four-part PE version,
  pinned config.xml and (when used) fixture SHA256 plus expected text-view length.
  Each trial uses fresh settings and an owned fixture copy. Flags force a new
  instance, disable plugins and omit session restore. The pinned configuration
  must disable updates (`noUpdate=yes`, absent or zero `autoUpdateMode`). Original
  fixtures are never saved or modified. Timeout terminates the owned Job.

Driver arguments: `--application {application} --sha256 <hash> --version <a.b.c.d>
--config <file> --config-sha256 <hash> --scenario <name>`. Fixture scenarios also
need `--fixture <file> --fixture-sha256 <hash> --expected-text-bytes <count>`.
The suite must pin this script and `windows_process_metrics.py` in `pinned_files`
as well as its Python executable. Run only in the explicitly scheduled benchmark
phase on an unlocked interactive desktop.

Supported scenario adapters: cold/warm launch, empty idle, open-size/long-line,
scroll, edit, literal/regex target search, result jump and Save. Their metric names
state the observable endpoints: responsive window/expected Scintilla text length,
message roundtrip, idle sample and sampled memory peak. `edit_to_paint` emits
`edit_roundtrip_us`, never a paint metric. Search measures Scintilla's target-search
primitive; it does not claim the Notepad++ Find dialog's cancellation or separate
regex implementation. Save modifies only the copied fixture and checks modified
state. Save also emits `save_to_clean_ack_us`, from dispatch to verified clean state
after the same PERF_SAVE insertion as Bareline. This shared acknowledgement endpoint
does not claim flush durability or paint completion.

Unsupported signals/scenarios: frame presentation/stall distribution, syntax
viewport-ready, Find-dialog cancellation acknowledgement, Save As dialog,
workspace scan, tail append, 100/500-tab automation and extension-runtime memory
comparisons. Unsupported scenario IDs fail argument parsing; no proxy values are
invented. Warm launch completes one unmeasured responsive launch first. Cold launch
requires the explicit pinned `cache_protocol.py` preparer contract after all copy/hash
work; its bounded evidence is embedded in raw output. No built-in OS-cache reset is
claimed. Caller review must validate the external preparation method. Paired metric
comparisons must match endpoint semantics, fixture/configuration and cache state.
`downloaded_bytes=0` counts the adapter's downloads only; it is not a network trace.
Owned fixture disk bytes exclude application installation and private config data.

Authoritative API references used for source implementation:
- [Notepad++ command arguments](https://github.com/notepad-plus-plus/npp-usermanual/blob/master/content/docs/command-prompt.md)
- [Notepad++ configuration parsing](https://github.com/notepad-plus-plus/notepad-plus-plus/blob/master/PowerEditor/src/Parameters.cpp)
- [Notepad++ menu command IDs](https://github.com/notepad-plus-plus/notepad-plus-plus/blob/master/PowerEditor/src/menuCmdID.h)
- [Scintilla API](https://www.scintilla.org/ScintillaDoc.html) and the repository's
  pinned Scintilla 5.5.7 header for numeric message IDs.

Required gate cases: wrong hashes/version/config rejected before launch; Job
assignment failure never runs uncontained; no-client/timeout cleanup; sampler
partial-access and exited-child rejection; safe fixture open/edit/undo/save;
search no-match and Unicode payload; exact expected-length mismatch; full owned
child-tree exit. No passing results are claimed here.
